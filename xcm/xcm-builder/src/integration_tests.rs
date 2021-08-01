// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use frame_support::{
	assert_ok,
	weights::Weight,
};
use sp_core::H256;
use sp_runtime::traits::AccountIdConversion;
use sp_runtime::{testing::Header, traits::IdentityLookup, AccountId32};

use polkadot_parachain::primitives::Id as ParaId;
use polkadot_runtime_parachains::{configuration, origin, shared};
use xcm::opaque::v0::prelude::*;
use xcm::opaque::v0::{MultiAsset, Response};
use xcm::v0::{Junction, MultiLocation::{self, *}, NetworkId, Order};
use xcm_executor::XcmExecutor;

use crate as xcm_builder;
use xcm_builder::{
	AccountId32Aliases, AllowTopLevelPaidExecutionFrom, ChildParachainAsNative,
	ChildParachainConvertsVia, ChildSystemParachainAsSuperuser,
	CurrencyAdapter as XcmCurrencyAdapter, FixedRateOfConcreteFungible, FixedWeightBounds,
	IsConcrete, LocationInverter, SignedAccountId32AsNative, SignedToAccountId32,
	SovereignSignedViaLocation, TakeWeightCredit, IsChildSystemParachain, AllowUnpaidExecutionFrom
};
use crate::mock;

use crate::integration_mock::{AccountId, Balances, BaseXcmWeight, ExistentialDeposit, kusama_like_with_balances, Origin, XcmConfig, XcmPallet};

pub const ALICE: AccountId = AccountId::new([0u8; 32]);
pub const PARA_ID: u32 = 2000;
pub const INITIAL_BALANCE: u128 = 100_000_000_000;

// Construct a `BuyExecution` order.
fn buy_execution<C>(debt: Weight) -> Order<C> {
	use xcm::opaque::v0::prelude::*;
	Order::BuyExecution {
		fees: All,
		weight: 0,
		debt,
		halt_on_error: false,
		xcm: vec![],
	}
}

/// Scenario:
/// A parachain transfers funds on the relaychain to another parachain's account.
///
/// Asserts that the parachain accounts are updated as expected.
#[test]
fn withdraw_and_deposit_works() {
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount =  10 * ExistentialDeposit::get();
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(Balances::free_balance(other_para_acc), amount);
	});
}

/// Scenario:
/// A user Alice sends funds from the relaychain to a parachain.
///
/// Asserts that the correct XCM is sent and the balances are set as expected.
#[test]
fn reserve_transfer_assets_works() {
	use xcm::opaque::v0::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let amount =  10 * ExistentialDeposit::get();
		// We just assume that the destination uses the same base weight for XCM for this test. Not checked.
		let dest_weight = 2 * BaseXcmWeight::get();
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Parachain(PARA_ID).into(),
			Junction::AccountId32 { network: NetworkId::Kusama, id: ALICE.into() }.into(),
			vec![ConcreteFungible { id: Null, amount }],
			dest_weight,
		));

		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - amount);
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm::ReserveAssetDeposit {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: vec![
						buy_execution(dest_weight),
						DepositAsset {
							assets: vec![All],
							dest: Junction::AccountId32 {
								network: NetworkId::Kusama,
								id: ALICE.into()
							}
							.into()
						},
					]
				}
			)]
		);
	});
}

/// Scenario:
/// A parachain wants to be notified that a transfer worked correctly.
/// It sends a `QueryHolding` after the deposit to get notified on success.
///
/// Asserts that the balances are updated correctly and the expected XCM is sent.
#[test]
fn query_holding_works() {
	use xcm::opaque::v0::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let other_para_id = 3000;
		let amount = 10 * ExistentialDeposit::get();
		let query_id = 1234;
		let weight = 4 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: OnlyChild.into(), // invalid destination
					},
					// is not triggered becasue the deposit fails
					Order::QueryHolding {
						query_id,
						dest: Parachain(PARA_ID).into(),
						assets: vec![All],
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Incomplete(weight, XcmError::FailedToTransactAsset("AccountIdConversionFailed")));
		// there should be no query response sent for the failed deposit
		assert_eq!(mock::sent_xcm(), vec![]);
		assert_eq!(Balances::free_balance(para_acc.clone()), INITIAL_BALANCE - amount);

		// now do a successful transfer
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(other_para_id).into(),
					},
					// used to get a notification in case of success
					Order::QueryHolding {
						query_id,
						dest: Parachain(PARA_ID).into(),
						assets: vec![All],
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		let other_para_acc: AccountId = ParaId::from(other_para_id).into_account();
		assert_eq!(Balances::free_balance(other_para_acc), amount);
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - 2 * amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(PARA_ID).into(),
				Xcm::QueryResponse {
					query_id,
					response: Response::Assets(vec![])
				}
			)]
		);
	});
}

/// Scenario:
/// A parachain wants to move KSM from Kusama to Statemine.
/// It withdraws funds and then teleports them to the destination.
///
/// Asserts that the balances are updated accordingly and the correct XCM is sent.
#[test]
fn teleport_to_statemine_works() {
	use xcm::opaque::v0::prelude::*;
	let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
	let balances = vec![(ALICE, INITIAL_BALANCE), (para_acc.clone(), INITIAL_BALANCE)];
	kusama_like_with_balances(balances).execute_with(|| {
		let statemine_id = 1000;
		let amount =  10 * ExistentialDeposit::get();
		let teleport_effects = vec![
			buy_execution(0),
			Order::DepositAsset { assets: vec![All], dest: Parachain(PARA_ID).into() },
		];
		let weight = 3 * BaseXcmWeight::get();
		let r = XcmExecutor::<XcmConfig>::execute_xcm(
			Parachain(PARA_ID).into(),
			Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::InitiateTeleport {
						assets: vec![All],
						dest: Parachain(statemine_id).into(),
						effects: teleport_effects.clone(),
					},
				],
			},
			weight,
		);
		assert_eq!(r, Outcome::Complete(weight));
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE - amount);
		assert_eq!(
			mock::sent_xcm(),
			vec![(
				Parachain(statemine_id).into(),
				Xcm::TeleportAsset {
					assets: vec![ConcreteFungible { id: Parent.into(), amount }],
					effects: teleport_effects,
				}
			)]
		);
	});
}
