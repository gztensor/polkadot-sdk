// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

#![cfg(test)]

use crate as pallet_xcm_bridge_hub;

use bp_messages::{
	target_chain::{DispatchMessage, MessageDispatch},
	ChainWithMessages, HashedLaneId, MessageNonce,
};
use bp_runtime::{messages::MessageDispatchResult, Chain, ChainId, HashOf};
use bp_xcm_bridge_hub::{BridgeId, BridgeLocations, LocalXcmChannelManager};
use codec::Encode;
use frame_support::{
	assert_ok, derive_impl, parameter_types,
	traits::{EnsureOrigin, Equals, Everything, OriginTrait},
	weights::RuntimeDbWeight,
};
use frame_support::traits::EitherOf;
use frame_support::traits::fungible::Mutate;
use frame_system::EnsureRootWithSuccess;
use polkadot_parachain_primitives::primitives::Sibling;
use sp_core::H256;
use sp_runtime::{
	testing::Header as SubstrateHeader,
	traits::{BlakeTwo256, ConstU128, ConstU32, IdentityLookup},
	AccountId32, BuildStorage, StateVersion,
};
use sp_std::cell::RefCell;
use xcm::{latest::ROCOCO_GENESIS_HASH, prelude::*};
use xcm_builder::{
	AllowUnpaidExecutionFrom, DispatchBlob, DispatchBlobError, FixedWeightBounds,
	InspectMessageQueues, NetworkExportTable, NetworkExportTableItem, ParentIsPreset,
	SiblingParachainConvertsVia, SovereignPaidRemoteExporter, UnpaidLocalExporter
};
use xcm_executor::traits::ConvertLocation;
use xcm_executor::XcmExecutor;

pub type AccountId = AccountId32;
pub type Balance = u64;
type Block = frame_system::mocking::MockBlock<TestRuntime>;

/// Lane identifier type used for tests.
pub type TestLaneIdType = HashedLaneId;

pub const SIBLING_ASSET_HUB_ID: u32 = 2001;
pub const THIS_BRIDGE_HUB_ID: u32 = 2002;
pub const BRIDGED_ASSET_HUB_ID: u32 = 1001;

frame_support::construct_runtime! {
	pub enum TestRuntime {
		System: frame_system,
		Balances: pallet_balances,
		Messages: pallet_bridge_messages,
		XcmOverBridge: pallet_xcm_bridge_hub,
		XcmOverBridgeWrappedWithExportMessageRouter: pallet_xcm_bridge_hub_router = 57,
		XcmOverBridgeByExportXcmRouter: pallet_xcm_bridge_hub_router::<Instance2> = 69,
	}
}

parameter_types! {
	pub const DbWeight: RuntimeDbWeight = RuntimeDbWeight { read: 1, write: 2 };
	pub const ExistentialDeposit: Balance = 1;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for TestRuntime {
	type AccountId = AccountId;
	type AccountData = pallet_balances::AccountData<Balance>;
	type Block = Block;
	type Lookup = IdentityLookup<Self::AccountId>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for TestRuntime {
	type AccountStore = System;
}

impl pallet_bridge_messages::Config for TestRuntime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = TestMessagesWeights;

	type ThisChain = ThisUnderlyingChain;
	type BridgedChain = BridgedUnderlyingChain;
	type BridgedHeaderChain = BridgedHeaderChain;

	type OutboundPayload = Vec<u8>;
	type InboundPayload = Vec<u8>;
	type LaneId = TestLaneIdType;

	type DeliveryPayments = ();
	type DeliveryConfirmationPayments = ();
	type OnMessagesDelivered = ();

	type MessageDispatch = TestMessageDispatch;
}

pub struct TestMessagesWeights;

impl pallet_bridge_messages::WeightInfo for TestMessagesWeights {
	fn receive_single_message_proof() -> Weight {
		Weight::zero()
	}
	fn receive_n_messages_proof(_: u32) -> Weight {
		Weight::zero()
	}
	fn receive_single_message_proof_with_outbound_lane_state() -> Weight {
		Weight::zero()
	}
	fn receive_single_n_bytes_message_proof(_: u32) -> Weight {
		Weight::zero()
	}
	fn receive_delivery_proof_for_single_message() -> Weight {
		Weight::zero()
	}
	fn receive_delivery_proof_for_two_messages_by_single_relayer() -> Weight {
		Weight::zero()
	}
	fn receive_delivery_proof_for_two_messages_by_two_relayers() -> Weight {
		Weight::zero()
	}
	fn receive_single_n_bytes_message_proof_with_dispatch(_n: u32) -> Weight {
		Weight::from_parts(1, 0)
	}
}

impl pallet_bridge_messages::WeightInfoExt for TestMessagesWeights {
	fn expected_extra_storage_proof_size() -> u32 {
		0
	}

	fn receive_messages_proof_overhead_from_runtime() -> Weight {
		Weight::zero()
	}

	fn receive_messages_delivery_proof_overhead_from_runtime() -> Weight {
		Weight::zero()
	}
}

parameter_types! {
	pub const HereLocation: Location = Location::here();
	pub const RelayNetwork: NetworkId = NetworkId::Kusama;
	pub UniversalLocation: InteriorLocation = [
		GlobalConsensus(RelayNetwork::get()),
		Parachain(THIS_BRIDGE_HUB_ID),
	].into();
	pub SiblingLocation: Location = Location::new(1, [Parachain(SIBLING_ASSET_HUB_ID)]);
	pub SiblingUniversalLocation: InteriorLocation = [GlobalConsensus(RelayNetwork::get()), Parachain(SIBLING_ASSET_HUB_ID)].into();

	pub const BridgedRelayNetwork: NetworkId = NetworkId::ByGenesis([1; 32]);
	pub BridgedRelayNetworkLocation: Location = (Parent, GlobalConsensus(BridgedRelayNetwork::get())).into();
	pub BridgedRelativeDestination: InteriorLocation = [Parachain(BRIDGED_ASSET_HUB_ID)].into();
	pub BridgedUniversalDestination: InteriorLocation = [GlobalConsensus(BridgedRelayNetwork::get()), Parachain(BRIDGED_ASSET_HUB_ID)].into();
	pub const NonBridgedRelayNetwork: NetworkId = NetworkId::ByGenesis(ROCOCO_GENESIS_HASH);

	pub const BridgeDeposit: Balance = 100_000;

	// configuration for pallet_xcm_bridge_hub_router
	pub BridgeHubLocation: Location = Here.into();
	pub BridgeFeeAsset: AssetId = Location::here().into();
	pub BridgeTable: Vec<NetworkExportTableItem>
		= vec![
			NetworkExportTableItem::new(
				BridgedRelayNetwork::get(),
				None,
				BridgeHubLocation::get(),
				None
			)
		];
	pub UnitWeightCost: Weight = Weight::from_parts(10, 10);
}

/// **Universal** `InteriorLocation` of bridged asset hub.
pub fn bridged_asset_hub_universal_location() -> InteriorLocation {
	BridgedUniversalDestination::get()
}

impl pallet_xcm_bridge_hub::Config for TestRuntime {
	type RuntimeEvent = RuntimeEvent;

	type UniversalLocation = UniversalLocation;
	type BridgedNetwork = BridgedRelayNetworkLocation;
	type BridgeMessagesPalletInstance = ();

	type MessageExportPrice = ();
	type DestinationVersion = AlwaysLatest;

	type ForceOrigin = frame_system::EnsureNever<()>;
	type OpenBridgeOrigin = EitherOf<
		// We want to translate `RuntimeOrigin::root()` to the `Location::here()`
		EnsureRootWithSuccess<AccountId, HereLocation>,
		OpenBridgeOrigin,
	>;
	type BridgeOriginAccountIdConverter = LocationToAccountId;

	type BridgeDeposit = BridgeDeposit;
	type Currency = Balances;
	type RuntimeHoldReason = RuntimeHoldReason;
	type AllowWithoutBridgeDeposit = (
		Equals<ParentRelayChainLocation>,
		Equals<HereLocation>
	);

	type LocalXcmChannelManager = TestLocalXcmChannelManager;

	type BlobDispatcher = TestBlobDispatcher;
}

/// A router instance simulates a scenario where the router is deployed on a different chain than
/// the `MessageExporter`. This means that the router sends an `ExportMessage`.
impl pallet_xcm_bridge_hub_router::Config<()> for TestRuntime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();

	type UniversalLocation = UniversalLocation;
	type SiblingBridgeHubLocation = BridgeHubLocation;
	type BridgedNetworkId = BridgedRelayNetwork;
	type Bridges = NetworkExportTable<BridgeTable>;
	type DestinationVersion = AlwaysLatest;

	// We use `SovereignPaidRemoteExporter` here to test and ensure that the `ExportMessage`
	// produced by `pallet_xcm_bridge_hub_router` is compatible with the `ExportXcm` implementation
	// of `pallet_xcm_bridge_hub`.
	type ToBridgeHubSender = SovereignPaidRemoteExporter<
		XcmOverBridgeWrappedWithExportMessageRouter,
		// **Note**: The crucial part is that `ExportMessage` is processed by `XcmExecutor`, which
		// calls the `ExportXcm` implementation of `pallet_xcm_bridge_hub` as the
		// `MessageExporter`.
		ExecuteXcmOverSendXcm,
		Self::UniversalLocation,
	>;
	type LocalXcmChannelManager = TestLocalXcmChannelManager;

	type ByteFee = ConstU128<0>;
	type FeeAsset = BridgeFeeAsset;
}

/// A router instance simulates a scenario where the router is deployed on the same chain than the `MessageExporter`. This means that the router triggers `ExportXcm` trait directly.
impl pallet_xcm_bridge_hub_router::Config<pallet_xcm_bridge_hub_router::Instance2> for TestRuntime {
	type RuntimeEvent = RuntimeEvent;
	type WeightInfo = ();

	type UniversalLocation = UniversalLocation;
	type SiblingBridgeHubLocation = BridgeHubLocation;
	type BridgedNetworkId = BridgedRelayNetwork;
	type Bridges = NetworkExportTable<BridgeTable>;
	type DestinationVersion = AlwaysLatest;

	// We use `UnpaidLocalExporter` here to test and ensure that `pallet_xcm_bridge_hub_router` can trigger directly `pallet_xcm_bridge_hub` as exporter.
	type ToBridgeHubSender = UnpaidLocalExporter<
		XcmOverBridge,
		Self::UniversalLocation,
	>;
	type LocalXcmChannelManager = TestLocalXcmChannelManager;

	type ByteFee = ConstU128<0>;
	type FeeAsset = BridgeFeeAsset;
}

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = RuntimeCall;
	type XcmSender = ();
	type AssetTransactor = ();
	type OriginConverter = ();
	type IsReserve = ();
	type IsTeleporter = ();
	type UniversalLocation = UniversalLocation;
	type Barrier = AllowUnpaidExecutionFrom<Everything>;
	type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, ConstU32<100>>;
	type Trader = ();
	type ResponseHandler = ();
	type AssetTrap = ();
	type AssetClaims = ();
	type SubscriptionService = ();
	type PalletInstancesInfo = ();
	type MaxAssetsIntoHolding = ();
	type AssetLocker = ();
	type AssetExchanger = ();
	type FeeManager = ();
	// We just set `MessageExporter` as our `pallet_xcm_bridge_hub` instance.
	type MessageExporter = (XcmOverBridge,);
	type UniversalAliases = ();
	type CallDispatcher = RuntimeCall;
	type SafeCallFilter = Everything;
	type Aliasers = ();
	type TransactionalProcessor = ();
	type HrmpNewChannelOpenRequestHandler = ();
	type HrmpChannelAcceptedHandler = ();
	type HrmpChannelClosingHandler = ();
	type XcmRecorder = ();
}

thread_local! {
	pub static EXECUTE_XCM_ORIGIN: RefCell<Option<Location>> = RefCell::new(None);
}

/// The `SendXcm` implementation directly executes XCM using `XcmExecutor`.
pub struct ExecuteXcmOverSendXcm;
impl SendXcm for ExecuteXcmOverSendXcm {
	type Ticket = Xcm<()>;

	fn validate(
		_: &mut Option<Location>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		Ok((message.take().unwrap(), Assets::new()))
	}

	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		let xcm: Xcm<RuntimeCall> = ticket.into();

		let origin = EXECUTE_XCM_ORIGIN.with(|o| o.borrow().clone().unwrap());
		let mut hash = xcm.using_encoded(sp_io::hashing::blake2_256);
		let outcome = XcmExecutor::<XcmConfig>::prepare_and_execute(
			origin,
			xcm,
			&mut hash,
			Weight::MAX,
			Weight::zero(),
		);
		assert_ok!(outcome.ensure_complete());

		Ok(hash)
	}
}
impl InspectMessageQueues for ExecuteXcmOverSendXcm {
	fn clear_messages() {
		todo!()
	}

	fn get_messages() -> Vec<(VersionedLocation, Vec<VersionedXcm<()>>)> {
		todo!()
	}
}
impl ExecuteXcmOverSendXcm {
	pub fn set_origin_for_execute(origin: Location) {
		EXECUTE_XCM_ORIGIN.with(|o| *o.borrow_mut() = Some(origin));
	}
}

/// Type for specifying how a `Location` can be converted into an `AccountId`. This is used
/// when determining ownership of accounts for asset transacting and when attempting to use XCM
/// `Transact` in order to determine the dispatch Origin.
pub type LocationToAccountId = (
	// The parent (Relay-chain) origin converts to the parent `AccountId`.
	ParentIsPreset<AccountId>,
	// Sibling parachain origins convert to AccountId via the `ParaId::into`.
	SiblingParachainConvertsVia<Sibling, AccountId>,
);

parameter_types! {
	pub ParentRelayChainLocation: Location = Location { parents: 1, interior: Here };
}
pub struct OpenBridgeOrigin;

impl OpenBridgeOrigin {
	pub fn parent_relay_chain_origin() -> RuntimeOrigin {
		RuntimeOrigin::signed([0u8; 32].into())
	}

	pub fn parent_relay_chain_universal_origin() -> RuntimeOrigin {
		RuntimeOrigin::signed([1u8; 32].into())
	}

	pub fn sibling_parachain_origin() -> RuntimeOrigin {
		let mut account = [0u8; 32];
		account[..4].copy_from_slice(&SIBLING_ASSET_HUB_ID.encode()[..4]);
		RuntimeOrigin::signed(account.into())
	}

	pub fn sibling_parachain_universal_origin() -> RuntimeOrigin {
		RuntimeOrigin::signed([2u8; 32].into())
	}

	pub fn origin_without_sovereign_account() -> RuntimeOrigin {
		RuntimeOrigin::signed([3u8; 32].into())
	}

	pub fn disallowed_origin() -> RuntimeOrigin {
		RuntimeOrigin::signed([42u8; 32].into())
	}
}

impl EnsureOrigin<RuntimeOrigin> for OpenBridgeOrigin {
	type Success = Location;

	fn try_origin(o: RuntimeOrigin) -> Result<Self::Success, RuntimeOrigin> {
		let signer = o.clone().into_signer();
		if signer == Self::parent_relay_chain_origin().into_signer() {
			return Ok(ParentRelayChainLocation::get())
		} else if signer == Self::parent_relay_chain_universal_origin().into_signer() {
			return Ok(Location {
				parents: 2,
				interior: GlobalConsensus(RelayNetwork::get()).into(),
			})
		} else if signer == Self::sibling_parachain_universal_origin().into_signer() {
			return Ok(Location {
				parents: 2,
				interior: [GlobalConsensus(RelayNetwork::get()), Parachain(SIBLING_ASSET_HUB_ID)]
					.into(),
			})
		} else if signer == Self::origin_without_sovereign_account().into_signer() {
			return Ok(Location {
				parents: 1,
				interior: [Parachain(SIBLING_ASSET_HUB_ID), OnlyChild].into(),
			});
		} else if signer == Self::sibling_parachain_origin().into_signer() {
			return Ok(SiblingLocation::get());
		}

		Err(o)
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn try_successful_origin() -> Result<RuntimeOrigin, ()> {
		Ok(Self::parent_relay_chain_origin())
	}
}

pub(crate) type OpenBridgeOriginOf<T, I> = <T as pallet_xcm_bridge_hub::Config<I>>::OpenBridgeOrigin;

pub(crate) fn fund_origin_sovereign_account(locations: &BridgeLocations, balance: Balance) -> AccountId {
	let bridge_owner_account =
		LocationToAccountId::convert_location(locations.bridge_origin_relative_location())
			.unwrap();
	assert_ok!(Balances::mint_into(&bridge_owner_account, balance));
	bridge_owner_account
}

pub struct TestLocalXcmChannelManager;

impl TestLocalXcmChannelManager {
	pub fn make_congested() {
		frame_support::storage::unhashed::put(b"TestLocalXcmChannelManager.Congested", &true);
	}

	pub fn is_bridge_suspened() -> bool {
		frame_support::storage::unhashed::get_or_default(b"TestLocalXcmChannelManager.Suspended")
	}

	pub fn is_bridge_resumed() -> bool {
		frame_support::storage::unhashed::get_or_default(b"TestLocalXcmChannelManager.Resumed")
	}
}

impl LocalXcmChannelManager for TestLocalXcmChannelManager {
	type Error = ();

	fn is_congested(_with: &Location) -> bool {
		frame_support::storage::unhashed::get_or_default(b"TestLocalXcmChannelManager.Congested")
	}

	fn suspend_bridge(_local_origin: &Location, _bridge: BridgeId) -> Result<(), Self::Error> {
		frame_support::storage::unhashed::put(b"TestLocalXcmChannelManager.Suspended", &true);
		Ok(())
	}

	fn resume_bridge(_local_origin: &Location, _bridge: BridgeId) -> Result<(), Self::Error> {
		frame_support::storage::unhashed::put(b"TestLocalXcmChannelManager.Resumed", &true);
		Ok(())
	}
}

impl pallet_xcm_bridge_hub_router::XcmChannelStatusProvider for TestLocalXcmChannelManager {
	fn is_congested(with: &Location) -> bool {
		<Self as LocalXcmChannelManager>::is_congested(with)
	}
}

pub struct TestBlobDispatcher;

impl TestBlobDispatcher {
	pub fn is_dispatched() -> bool {
		frame_support::storage::unhashed::get_or_default(b"TestBlobDispatcher.Dispatched")
	}
}

impl DispatchBlob for TestBlobDispatcher {
	fn dispatch_blob(_blob: Vec<u8>) -> Result<(), DispatchBlobError> {
		frame_support::storage::unhashed::put(b"TestBlobDispatcher.Dispatched", &true);
		Ok(())
	}
}

pub struct ThisUnderlyingChain;

impl Chain for ThisUnderlyingChain {
	const ID: ChainId = *b"tuch";

	type BlockNumber = u64;
	type Hash = H256;
	type Hasher = BlakeTwo256;
	type Header = SubstrateHeader;
	type AccountId = AccountId;
	type Balance = Balance;
	type Nonce = u64;
	type Signature = sp_runtime::MultiSignature;

	const STATE_VERSION: StateVersion = StateVersion::V1;

	fn max_extrinsic_size() -> u32 {
		u32::MAX
	}

	fn max_extrinsic_weight() -> Weight {
		Weight::MAX
	}
}

impl ChainWithMessages for ThisUnderlyingChain {
	const WITH_CHAIN_MESSAGES_PALLET_NAME: &'static str = "WithThisChainBridgeMessages";
	const MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX: MessageNonce = 16;
	const MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX: MessageNonce = 128;
}

pub type BridgedHeaderHash = H256;
pub type BridgedChainHeader = SubstrateHeader;

pub struct BridgedUnderlyingChain;
impl Chain for BridgedUnderlyingChain {
	const ID: ChainId = *b"bgdc";
	type BlockNumber = u64;
	type Hash = BridgedHeaderHash;
	type Hasher = BlakeTwo256;
	type Header = BridgedChainHeader;
	type AccountId = AccountId;
	type Balance = Balance;
	type Nonce = u64;
	type Signature = sp_runtime::MultiSignature;

	const STATE_VERSION: StateVersion = StateVersion::V1;

	fn max_extrinsic_size() -> u32 {
		4096
	}

	fn max_extrinsic_weight() -> Weight {
		Weight::MAX
	}
}

impl ChainWithMessages for BridgedUnderlyingChain {
	const WITH_CHAIN_MESSAGES_PALLET_NAME: &'static str = "WithBridgedChainBridgeMessages";
	const MAX_UNREWARDED_RELAYERS_IN_CONFIRMATION_TX: MessageNonce = 16;
	const MAX_UNCONFIRMED_MESSAGES_IN_CONFIRMATION_TX: MessageNonce = 128;
}

pub struct BridgedHeaderChain;
impl bp_header_chain::HeaderChain<BridgedUnderlyingChain> for BridgedHeaderChain {
	fn finalized_header_state_root(
		_hash: HashOf<BridgedUnderlyingChain>,
	) -> Option<HashOf<BridgedUnderlyingChain>> {
		unreachable!()
	}
}

/// Test message dispatcher.
pub struct TestMessageDispatch;

impl TestMessageDispatch {
	pub fn deactivate(lane: TestLaneIdType) {
		frame_support::storage::unhashed::put(&(b"inactive", lane).encode()[..], &false);
	}
}

impl MessageDispatch for TestMessageDispatch {
	type DispatchPayload = Vec<u8>;
	type DispatchLevelResult = ();
	type LaneId = TestLaneIdType;

	fn is_active(lane: Self::LaneId) -> bool {
		frame_support::storage::unhashed::take::<bool>(&(b"inactive", lane).encode()[..]) !=
			Some(false)
	}

	fn dispatch_weight(
		_message: &mut DispatchMessage<Self::DispatchPayload, Self::LaneId>,
	) -> Weight {
		Weight::zero()
	}

	fn dispatch(
		_: DispatchMessage<Self::DispatchPayload, Self::LaneId>,
	) -> MessageDispatchResult<Self::DispatchLevelResult> {
		MessageDispatchResult { unspent_weight: Weight::zero(), dispatch_level_result: () }
	}
}

/// Run pallet test.
pub fn run_test<T>(test: impl FnOnce() -> T) -> T {
	sp_io::TestExternalities::new(
		frame_system::GenesisConfig::<TestRuntime>::default().build_storage().unwrap(),
	)
	.execute_with(test)
}
