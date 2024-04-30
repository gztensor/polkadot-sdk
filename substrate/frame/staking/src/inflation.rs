// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Staking inflation pallet.
//!
//! This pallet provides inflation related functionality specifically for
//! (`pallet-staking``)[`crate`]. While generalized to a high extent, it is not necessarily written
//! to be reusable outside of the Polkadot relay chain scope.
//!
//! This pallet processes inflation in the following steps: :
//!
//! 1. [`Config::MaxInflation`] is always minted, and it is split into two portions: `staking` and
//!    `leftover` based on [`Config::IdealStakingRate`].
//! assci diagram that show
//! ```
//! |-----------|---------------------------------|
//!   leftover               staking
//! ```
//! 2. First, [`Config::PreStakingRecipients`] are paid out. This

use sp_runtime::{curve::PiecewiseLinear, traits::AtLeast32BitUnsigned, Perbill};

#[frame_support::pallet]
pub mod polkadot_inflation {
	//! Polkadot inflation pallet.
	use frame_support::{
		pallet_prelude::*,
		traits::{
			fungible::{self as fung, Inspect, Mutate},
			UnixTime,
		},
	};
	use frame_system::pallet_prelude::*;
	use sp_runtime::{traits::Saturating, Perquintill};

	type BalanceOf<T> = <T as Config>::CurrencyBalance;

	const MILLISECONDS_PER_YEAR: u64 = 1000 * 3600 * 24 * 36525 / 100;

	#[pallet::pallet]
	pub struct Pallet<T>(_);

	/// Default implementations of [`DefaultConfig`], which can be used to implement [`Config`].
	// pub mod config_preludes {
	// 	use super::*;
	// 	use frame_support::derive_impl;

	// 	type AccountId = <TestDefaultConfig as frame_system::DefaultConfig>::AccountId;

	// 	pub struct TestDefaultConfig;

	// 	#[derive_impl(frame_system::config_preludes::TestDefaultConfig, no_aggregated_types)]
	// 	impl frame_system::DefaultConfig for TestDefaultConfig {}

	// 	frame_support::parameter_types! {
	// 		pub const IdealStakingRate: Perquintill = Perquintill::from_percent(75);
	// 		pub const MaxInflation: Perquintill = Perquintill::from_percent(10);
	// 		pub const MinInflation: Perquintill = Perquintill::from_percent(2);
	// 		pub const Falloff: Perquintill = Perquintill::from_percent(5);
	// 		pub const LeftoverRecipients: Vec<(AccountId, Perquintill)> = vec![];
	// 	}

	// 	use crate::inflation::polkadot_inflation::DefaultConfig;
	// 	#[frame_support::register_default_impl(TestDefaultConfig)]
	// 	impl DefaultConfig for TestDefaultConfig {
	// 		#[inject_runtime_type]
	// 		type RuntimeEvent = ();

	// 		type IdealStakingRate = IdealStakingRate;
	// 		type MaxInflation = MaxInflation;
	// 		type MinInflation = MinInflation;
	// 		type Falloff = Falloff;
	// 		type LeftoverRecipients = LeftoverRecipients;
	// 	}
	// }

	enum InflationPayout<AccountId> {
		/// Pay the amount to the given account.
		Pay(AccountId),
		/// Split the equally between the given accounts.
		///
		/// This can always be implemented by a combination of [`Self::Pay`], but it is easier to express things like "split the amount between A, B, and C".
		SplitEqual(Vec<AccountId>)
		/// Burn the full amount.
		Burn,
	}

	struct Inflation<T> {
		max: BalanceOf<T>,
		staking: BalanceOf<T>,
	}

	impl<T: Config> Inflation<T> {
		fn new(max: BalanceOf<T>, staking: BalanceOf<T>) -> Self {
			Self { max, staking }
		}

		fn leftover(&self) -> BalanceOf<T> {
			self.max.saturating_sub(self.staking)
		}

		fn payout(&self, portion: Perquintill, recipient: InflationPayout) {
			let amount = portion * self.staking;
			match recipient {
				InflationPayout::Pay(who) => T::Currency::mint_into(&who, amount).defensive(),
				InflationPayout::Burn => T::Currency::burn(amount).defensive(),
			}
		}
	}

	#[pallet::config(with_default)]
	pub trait Config: frame_system::Config {
		#[pallet::no_default_bounds]
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		#[pallet::no_default]
		type UnixTime: frame_support::traits::UnixTime;

		#[pallet::no_default]
		type Recipients: Get<Vec<(InflationPayout<Self::AccountId>, Box<dyn FnOnce(BalanceOf<Self>) -> BalanceOf<Self>)>>

		/// Account Id to which staking inflation is forwarded to.
		#[pallet::no_default]
		type StakingRecipient: Get<Self::AccountId>;

		/// Ideal staking rate. Combined with [`Config::MaxInflation`], [`Config::MinInflation`] and
		/// [`Config::Falloff`] to determine the inflation rate that is forwarded to
		/// [`StakingRecipient`] account.
		type IdealStakingRate: Get<Perquintill>;
		type MaxInflation: Get<Perquintill>;
		type MinInflation: Get<Perquintill>;
		type Falloff: Get<Perquintill>;

		/// A list of recipients of [`Config::MaxInflation`] that front-run whatever is paid to
		/// [`StakingRecipient`].
		type PreStakingRecipients: Get<Vec<(Self::AccountId, Perquintill)>>;

		/// A list of recipients that get the leftovers of the total inflation.
		type LeftoverRecipients: Get<Vec<(Self::AccountId, Perquintill)>>;

		#[pallet::no_default]
		type Currency: fung::Mutate<Self::AccountId>
			+ fung::Inspect<Self::AccountId, Balance = Self::CurrencyBalance>;

		#[pallet::no_default]
		type CurrencyBalance: frame_support::traits::tokens::Balance + From<u64>;

		/// Customize how this pallet reads the total issuance, if need be.
		///
		/// This is mainly here to cater for Nis in Kusama.
		///
		/// NOTE: one should not use `T::Currency::total_issuance()` directly within the pallet in
		/// case it has been overwritten here.
		#[pallet::no_default]
		fn adjusted_total_issuance() -> BalanceOf<Self> {
			Self::Currency::total_issuance()
		}

		/// A simple and possibly short terms means for updating the total stake, esp. so long as
		/// this pallet is in the same runtime as with `pallet-staking`.
		///
		/// Once multi-chain, we should expect an extrinsic, gated by the origin of the staking
		/// parachain that can update this value. This can be `Transact`-ed via XCM.
		#[pallet::no_default] // TODO @gupnik this should be taken care of better? the fn already has a default.
		fn update_total_stake(stake: BalanceOf<Self>, valid_until: Option<BlockNumberFor<Self>>) {
			LastKnownStakedStorage::<Self>::put(LastKnownStake { stake, valid_until });
		}
	}

	// TODO: needs a migration that sets the initial value.
	// TODO: test if this is not set, that we are still bound to max inflation.
	#[pallet::storage]
	pub type LastInflated<T> = StorageValue<Value = u64, QueryKind = ValueQuery>;

	#[derive(Clone, Eq, PartialEq, DebugNoBound, Encode, Decode, TypeInfo, MaxEncodedLen)]
	#[scale_info(skip_type_params(T))]
	#[codec(mel_bound())]
	pub struct LastKnownStake<T: Config> {
		pub(crate) stake: BalanceOf<T>,
		pub(crate) valid_until: Option<BlockNumberFor<T>>,
	}

	// SHOULD ONLY BE READ BY [`Pallet::last_known_stake`]
	#[pallet::storage]
	type LastKnownStakedStorage<T: Config> =
		StorageValue<Value = LastKnownStake<T>, QueryKind = OptionQuery>;

	impl<T: Config> Pallet<T> {
		fn last_known_stake() -> Option<BalanceOf<T>> {
			LastKnownStakedStorage::<T>::get().and_then(|LastKnownStake { stake, valid_until }| {
				if valid_until.map_or(false, |valid_until| {
					valid_until < frame_system::Pallet::<T>::block_number()
				}) {
					None
				} else {
					Some(stake)
				}
			})
		}
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Inflated { staking: BalanceOf<T>, leftovers: BalanceOf<T> },
	}

	#[pallet::error]
	pub enum Error<T> {
		UnknownLastStake,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// `force_inflate`
		#[pallet::weight(0)]
		#[pallet::call_index(0)]
		pub fn force_inflate(origin: OriginFor<T>) -> DispatchResult {
			ensure_root(origin)?;
			Self::inflate_with_bookkeeping()?;
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Trigger an inflation,
		pub fn inflate_with_duration(
			since_last_inflation: u64,
		) -> Result<Inflation<BalanceOf<T>>, Error<T>> {
			let adjusted_total_issuance = T::adjusted_total_issuance();

			// what percentage of a year has passed since last inflation?
			let annual_proportion =
				Perquintill::from_rational(since_last_inflation, MILLISECONDS_PER_YEAR);

			let total_staked = Self::last_known_stake().ok_or(Error::<T>::UnknownLastStake)?;

			let min_annual_inflation = T::MinInflation::get();
			let max_annual_inflation = T::MaxInflation::get();
			let delta_annual_inflation = max_annual_inflation.saturating_sub(min_annual_inflation);
			let ideal_stake = T::IdealStakingRate::get();

			let staked_ratio = Perquintill::from_rational(total_staked, adjusted_total_issuance);
			let falloff = T::Falloff::get();

			let adjustment =
				pallet_staking_reward_fn::compute_inflation(staked_ratio, ideal_stake, falloff);
			let staking_annual_inflation: Perquintill =
				min_annual_inflation.saturating_add(delta_annual_inflation * adjustment);

			// final inflation formula.
			let payout_with_annual_inflation = |i| annual_proportion * i * adjusted_total_issuance;

			// ideal amount that we want to payout.
			let max_payout = payout_with_annual_inflation(max_annual_inflation);
			let staking_inflation = payout_with_annual_inflation(staking_annual_inflation);

			Ok(Inflation::new(max_payout, staking_inflation))
		}

		pub fn inflate_with_bookkeeping() -> DispatchResult {
			let last_inflated = LastInflated::<T>::get();
			let now = T::UnixTime::now().as_millis().saturated_into::<u64>();
			let since_last_inflation = now.saturating_sub(last_inflated);
			let inflation = Self::inflate_with_duration(since_last_inflation)?;

			// distribute the inflation to pots.
			T::Currency::mint_into(
				&T::StakingRecipient::get(),
				staking + T::Currency::minimum_balance(),
			)
			.defensive();
			T::LeftoverRecipients::get().into_iter().for_each(|(who, proportion)| {
				let amount = proportion * leftovers;
				T::Currency::mint_into(&who, amount).defensive();
			});

			crate::log!(
				debug,
				"inflation: done at {:?}, period duration: {:?}, staking: {:?}, leftovers: {:?}",
				now,
				since_last_inflation,
				staking,
				leftovers
			);

			Self::deposit_event(Event::Inflated { staking, leftovers });
			LastInflated::<T>::put(T::UnixTime::now().as_millis().saturated_into::<u64>());

			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::mock::*;

	#[test]
	fn inflation_stateless_is_sensible() {
		// standalone functions to make sure internal logic is sensible.
		// mostly a wrapper for `pallet_staking_reward_fn::compute_inflation`.
	}

	#[test]
	fn unset_last_known_total_stake() {
		// if unset, we should not inflate at ll.
	}

	#[test]
	fn expired_last_known_total_stake() {
		// if expired, we should not inflate at all.
	}

	#[test]
	fn inflation_is_time_independent() {
		// over a fixed period, eg. a day, total amount inflated is the same if we inflate every
		// block or every our or just once, assuming total stake is constant.
	}

	#[test]
	fn staking_inflation_works_with_zero_ed() {
		// inflation for staking, and how the stake is distributed into sub accounts is correct for
		// both zero and non-zero ED.
	}

	#[test]
	fn payouts_are_stored_in_pots() {
		// as we progress eras but no one claims, amounts are stored in pot accounts.
	}

	#[test]
	fn unclaimed_rewards_are_burnt() {
		// upon expiry, unclaimed rewards are burnt.
	}
}

mod deprecated {
	use super::*;

	/// The total payout to all validators (and their nominators) per era and maximum payout.
	///
	/// Defined as such:
	/// `staker-payout = yearly_inflation(npos_token_staked / total_tokens) * total_tokens /
	/// era_per_year` `maximum-payout = max_yearly_inflation * total_tokens / era_per_year`
	///
	/// `era_duration` is expressed in millisecond.
	#[deprecated]
	pub fn compute_total_payout<N>(
		yearly_inflation: &PiecewiseLinear<'static>,
		npos_token_staked: N,
		total_tokens: N,
		era_duration: u64,
	) -> (N, N)
	where
		N: AtLeast32BitUnsigned + Clone,
	{
		// Milliseconds per year for the Julian year (365.25 days).
		const MILLISECONDS_PER_YEAR: u64 = 1000 * 3600 * 24 * 36525 / 100;

		let portion = Perbill::from_rational(era_duration as u64, MILLISECONDS_PER_YEAR);
		let payout = portion *
			yearly_inflation.calculate_for_fraction_times_denominator(
				npos_token_staked,
				total_tokens.clone(),
			);
		let maximum = portion * (yearly_inflation.maximum * total_tokens);
		(payout, maximum)
	}

	#[cfg(test)]
	mod test {
		use sp_runtime::curve::PiecewiseLinear;

		pallet_staking_reward_curve::build! {
			const I_NPOS: PiecewiseLinear<'static> = curve!(
				min_inflation: 0_025_000,
				max_inflation: 0_100_000,
				ideal_stake: 0_500_000,
				falloff: 0_050_000,
				max_piece_count: 40,
				test_precision: 0_005_000,
			);
		}

		#[test]
		fn npos_curve_is_sensible() {
			const YEAR: u64 = 365 * 24 * 60 * 60 * 1000;

			// check maximum inflation.
			// not 10_000 due to rounding error.
			assert_eq!(super::compute_total_payout(&I_NPOS, 0, 100_000u64, YEAR).1, 9_993);

			// super::I_NPOS.calculate_for_fraction_times_denominator(25, 100)
			assert_eq!(super::compute_total_payout(&I_NPOS, 0, 100_000u64, YEAR).0, 2_498);
			assert_eq!(super::compute_total_payout(&I_NPOS, 5_000, 100_000u64, YEAR).0, 3_248);
			assert_eq!(super::compute_total_payout(&I_NPOS, 25_000, 100_000u64, YEAR).0, 6_246);
			assert_eq!(super::compute_total_payout(&I_NPOS, 40_000, 100_000u64, YEAR).0, 8_494);
			assert_eq!(super::compute_total_payout(&I_NPOS, 50_000, 100_000u64, YEAR).0, 9_993);
			assert_eq!(super::compute_total_payout(&I_NPOS, 60_000, 100_000u64, YEAR).0, 4_379);
			assert_eq!(super::compute_total_payout(&I_NPOS, 75_000, 100_000u64, YEAR).0, 2_733);
			assert_eq!(super::compute_total_payout(&I_NPOS, 95_000, 100_000u64, YEAR).0, 2_513);
			assert_eq!(super::compute_total_payout(&I_NPOS, 100_000, 100_000u64, YEAR).0, 2_505);

			const DAY: u64 = 24 * 60 * 60 * 1000;
			assert_eq!(super::compute_total_payout(&I_NPOS, 25_000, 100_000u64, DAY).0, 17);
			assert_eq!(super::compute_total_payout(&I_NPOS, 50_000, 100_000u64, DAY).0, 27);
			assert_eq!(super::compute_total_payout(&I_NPOS, 75_000, 100_000u64, DAY).0, 7);

			const SIX_HOURS: u64 = 6 * 60 * 60 * 1000;
			assert_eq!(super::compute_total_payout(&I_NPOS, 25_000, 100_000u64, SIX_HOURS).0, 4);
			assert_eq!(super::compute_total_payout(&I_NPOS, 50_000, 100_000u64, SIX_HOURS).0, 7);
			assert_eq!(super::compute_total_payout(&I_NPOS, 75_000, 100_000u64, SIX_HOURS).0, 2);

			const HOUR: u64 = 60 * 60 * 1000;
			assert_eq!(
				super::compute_total_payout(
					&I_NPOS,
					2_500_000_000_000_000_000_000_000_000u128,
					5_000_000_000_000_000_000_000_000_000u128,
					HOUR
				)
				.0,
				57_038_500_000_000_000_000_000
			);
		}
	}
}

pub use deprecated::compute_total_payout;
