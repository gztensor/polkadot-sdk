// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! JSON-RPC helpers.

use crate::SubscriptionTaskExecutor;
use futures::{
	future::{self, Either, Fuse, FusedFuture},
	Future, FutureExt, Stream, StreamExt,
};
use jsonrpsee::{ConnectionId, PendingSubscriptionSink, SubscriptionMessage, SubscriptionSink};
use sp_runtime::Serialize;
use std::{collections::VecDeque, net::IpAddr};

const DEFAULT_BUF_SIZE: usize = 16;

/// Parameters for a subscription.
pub struct SubscriptionParams {
	/// Connection id.
	pub conn_id: ConnectionId,
	/// Metrics.
	pub metrics: SubscriptionMetrics,
	/// Method name.
	pub method: &'static str,
	/// IP address of the peer.
	pub ip_addr: std::net::IpAddr,
}

#[derive(Debug, Clone)]
struct InnerMetrics {
	dropped_subscriptions: prometheus::CounterVec,
}

/// Metrics for dropped subscriptions.
#[derive(Debug, Clone)]
pub struct SubscriptionMetrics(Option<InnerMetrics>);

impl SubscriptionMetrics {
	/// Create a new metrics instance.
	pub fn new(
		registry: Option<prometheus::Registry>,
	) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
		if let Some(registry) = registry {
			let dropped_subscriptions = prometheus::CounterVec::new(
				prometheus::Opts::new(
					"substrate_rpc_dropped_subscriptions",
					"Number of subscriptions that was closed pre-maturely by the server due to lagging",
				),
				&["method", "peer"],
			)?;
			registry.register(Box::new(dropped_subscriptions.clone()))?;
			Ok(Self(Some(InnerMetrics { dropped_subscriptions })))
		} else {
			Ok(Self(None))
		}
	}

	/// Create a new metrics instance that is disabled.
	pub fn disabled() -> Self {
		Self(None)
	}

	/// Register that a subscription was dropped.
	pub fn register_dropped(&self, method: &str, peer: IpAddr) {
		if let Some(metrics) = &self.0 {
			metrics
				.dropped_subscriptions
				.with_label_values(&[method, &peer.to_string()])
				.inc();
		}
	}
}

/// A simple bounded VecDeque.
struct BoundedVecDeque<T> {
	inner: VecDeque<T>,
	max_cap: usize,
}

impl<T> BoundedVecDeque<T> {
	/// Create a new bounded VecDeque.
	fn new() -> Self {
		Self { inner: VecDeque::with_capacity(DEFAULT_BUF_SIZE), max_cap: DEFAULT_BUF_SIZE }
	}

	fn push_back(&mut self, item: T) -> Result<(), ()> {
		if self.inner.len() >= self.max_cap {
			Err(())
		} else {
			self.inner.push_back(item);
			Ok(())
		}
	}

	fn pop_front(&mut self) -> Option<T> {
		self.inner.pop_front()
	}
}

/// Feed items to the subscription from the underlying stream.
///
/// This is bounded because the underlying streams in substrate are
/// unbounded and if the subscription can't keep with stream it can
/// cause the buffer to become very large and consume lots of memory.
///
/// In such cases the subscription is dropped.
pub async fn pipe_from_stream<S, T>(
	pending: PendingSubscriptionSink,
	mut stream: S,
	params: SubscriptionParams,
) where
	S: Stream<Item = T> + Unpin + Send + 'static,
	T: Serialize + Send + 'static,
{
	let mut buf = BoundedVecDeque::new();
	let accept_fut = pending.accept();

	futures::pin_mut!(accept_fut);

	// Poll the stream while waiting for the subscription to be accepted
	//
	// If the `max_cap` is exceeded then the subscription is dropped.
	let sink = loop {
		match future::select(accept_fut, stream.next()).await {
			Either::Left((Ok(sink), _)) => break sink,
			Either::Right((Some(msg), f)) => {
				if buf.push_back(msg).is_err() {
					log::trace!(target: "rpc", "Subscription::accept lagged, dropping subscription=`{}`, peer=`{}`", params.method, params.ip_addr);
					params.metrics.register_dropped(params.method, params.ip_addr);
					return
				}
				accept_fut = f;
			},
			// The connection was closed or the stream was closed.
			_ => return,
		}
	};

	inner_pipe_from_stream(sink, stream, buf, params).await
}

async fn inner_pipe_from_stream<S, T>(
	sink: SubscriptionSink,
	mut stream: S,
	mut buf: BoundedVecDeque<T>,
	params: SubscriptionParams,
) where
	S: Stream<Item = T> + Unpin + Send + 'static,
	T: Serialize + Send + 'static,
{
	let mut next_fut = Box::pin(Fuse::terminated());
	let mut next_item = stream.next();
	let closed = sink.closed();
	let SubscriptionParams { metrics, method, ip_addr, .. } = params;

	futures::pin_mut!(closed);

	loop {
		if next_fut.is_terminated() {
			if let Some(v) = buf.pop_front() {
				let val = to_sub_message(&sink, &v);
				next_fut.set(async { sink.send(val).await }.fuse());
			}
		}

		match future::select(closed, future::select(next_fut, next_item)).await {
			// Send operation finished.
			Either::Right((Either::Left((_, n)), c)) => {
				next_item = n;
				closed = c;
				next_fut = Box::pin(Fuse::terminated());
			},
			// New item from the stream
			Either::Right((Either::Right((Some(v), n)), c)) => {
				if buf.push_back(v).is_err() {
					log::trace!(target: "rpc", "Subscription lagged, dropping subscription=`{}`, peer=`{}`", params.method, params.ip_addr);
					metrics.register_dropped(method, ip_addr);
					return
				}

				next_fut = n;
				closed = c;
				next_item = stream.next();
			},
			// Stream "finished".
			//
			// Process remaining items and terminate.
			Either::Right((Either::Right((None, pending_fut)), _)) => {
				if !pending_fut.is_terminated() && pending_fut.await.is_err() {
					return;
				}

				while let Some(v) = buf.pop_front() {
					let val = to_sub_message(&sink, &v);
					if sink.send(val).await.is_err() {
						return;
					}
				}

				return;
			},
			// Subscription was closed.
			Either::Left(_) => return,
		}
	}
}

/// Builds a subscription message.
///
/// # Panics
///
/// This function panics `Serialize` fails and it is treated a bug.
pub fn to_sub_message(sink: &SubscriptionSink, result: &impl Serialize) -> SubscriptionMessage {
	SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), result)
		.expect("Serialize infallible; qed")
}

/// Helper for spawning non-blocking rpc subscription task.
pub fn spawn_subscription_task(
	executor: &SubscriptionTaskExecutor,
	fut: impl Future<Output = ()> + Send + 'static,
) {
	executor.spawn("substrate-rpc-subscription", Some("rpc"), fut.boxed());
}

#[cfg(test)]
mod tests {
	use super::{pipe_from_stream, SubscriptionMetrics, SubscriptionParams};
	use futures::StreamExt;
	use jsonrpsee::{core::EmptyServerParams, ConnectionId, RpcModule, Subscription};

	fn sub_params() -> SubscriptionParams {
		SubscriptionParams {
			method: "sub",
			ip_addr: std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
			conn_id: ConnectionId(0),
			metrics: SubscriptionMetrics::disabled(),
		}
	}

	async fn subscribe() -> Subscription {
		let mut module = RpcModule::new(());
		module
			.register_subscription("sub", "my_sub", "unsub", |_, pending, _, _| async move {
				let stream = futures::stream::iter([0; 16]);
				pipe_from_stream(pending, stream, sub_params()).await;
				Ok(())
			})
			.unwrap();

		module.subscribe("sub", EmptyServerParams::new(), 1).await.unwrap()
	}

	#[tokio::test]
	async fn pipe_from_stream_works() {
		let mut sub = subscribe().await;
		let mut rx = 0;

		while let Some(Ok(_)) = sub.next::<usize>().await {
			rx += 1;
		}

		assert_eq!(rx, 16);
	}

	#[tokio::test]
	async fn pipe_from_stream_is_bounded() {
		let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();

		let mut module = RpcModule::new(tx);
		module
			.register_subscription("sub", "my_sub", "unsub", |_, pending, ctx, _| async move {
				let stream = futures::stream::iter([0; 32]);
				pipe_from_stream(pending, stream, sub_params()).await;
				_ = ctx.unbounded_send(());
				Ok(())
			})
			.unwrap();

		let mut sub = module.subscribe("sub", EmptyServerParams::new(), 1).await.unwrap();

		// When the 17th item arrives the subscription is dropped
		_ = rx.next().await.unwrap();
		assert!(sub.next::<usize>().await.is_none());
	}

	#[tokio::test]
	async fn subscription_is_dropped_when_stream_is_empty() {
		let notify_rx = std::sync::Arc::new(tokio::sync::Notify::new());
		let notify_tx = notify_rx.clone();

		let mut module = RpcModule::new(notify_tx);
		module
			.register_subscription(
				"sub",
				"my_sub",
				"unsub",
				|_, pending, notify_tx, _| async move {
					// emulate empty stream for simplicity: otherwise we need some mechanism
					// to sync buffer and channel send operations
					let stream = futures::stream::empty::<()>();
					// this should exit immediately
					pipe_from_stream(pending, stream, sub_params()).await;
					// notify that the `pipe_from_stream` has returned
					notify_tx.notify_one();
					Ok(())
				},
			)
			.unwrap();
		module.subscribe("sub", EmptyServerParams::new(), 1).await.unwrap();

		// it should fire once `pipe_from_stream` returns
		notify_rx.notified().await;
	}
}
