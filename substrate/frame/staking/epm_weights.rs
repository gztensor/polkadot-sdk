
//! Autogenerated weights for `pallet_epm_core`
//!
//! THIS FILE WAS AUTO-GENERATED USING THE SUBSTRATE BENCHMARK CLI VERSION 32.0.0
//! DATE: 2024-07-31, STEPS: `50`, REPEAT: `20`, LOW RANGE: `[]`, HIGH RANGE: `[]`
//! WORST CASE MAP SIZE: `1000000`
//! HOSTNAME: `gpestanas-MacBook-Pro.local`, CPU: `<UNKNOWN>`
//! WASM-EXECUTION: `Compiled`, CHAIN: `Some("dev")`, DB CACHE: 1024

// Executed Command:
// /Users/gpestana/cargo_target/debug/staking-node
// benchmark
// pallet
// --execution
// wasm
// --wasm-execution
// compiled
// --chain
// dev
// --pallet
// pallet-epm-core
// --extrinsic
// *
// --output
// epm_weights.rs

#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]
#![allow(missing_docs)]

use frame_support::{traits::Get, weights::Weight};
use core::marker::PhantomData;

/// Weight functions for `pallet_epm_core`.
pub struct WeightInfo<T>(PhantomData<T>);
impl<T: frame_system::Config> pallet_epm_core::WeightInfo for WeightInfo<T> {
	/// The range of component `t` is `[100, 500]`.
	fn create_targets_snapshot_page(_t: u32, ) -> Weight {
		// Proof Size summary in bytes:
		//  Measured:  `0`
		//  Estimated: `0`
		// Minimum execution time: 2_000_000 picoseconds.
		Weight::from_parts(2_984_267, 0)
			.saturating_add(Weight::from_parts(0, 0))
	}
}
