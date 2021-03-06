use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

use bitflags;
use byteorder::{BigEndian, WriteBytesExt};
use log::debug;
use secp256k1::{key::PublicKey, recovery::RecoverableSignature, recovery::RecoveryId, Signature};

use ckb_sdk::wallet::{
    is_valid_derivation_path, AbstractKeyStore, AbstractMasterPrivKey, AbstractPrivKey,
    ChildNumber, DerivationPath, ScryptType,
};
use ckb_sdk::SignEntireHelper;
use ckb_types::H256;

use ledger::ApduCommand;
use ledger::LedgerApp as RawLedgerApp;

pub mod apdu;
mod error;
pub mod parse;

pub use error::Error as LedgerKeyStoreError;

use ckb_types::{
    packed::{AnnotatedTransaction, Bip32, Uint32},
    prelude::*,
};

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}

pub struct LedgerKeyStore {
    discovered_devices: HashMap<LedgerId, LedgerMasterCap>,
}

#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
// TODO make contain actual id to distinguish between ledgers
pub struct LedgerId(pub H256);

impl LedgerKeyStore {
    fn new() -> Self {
        LedgerKeyStore {
            discovered_devices: HashMap::new(),
        }
    }

    fn refresh(&mut self) -> Result<(), LedgerKeyStoreError> {
        self.discovered_devices.clear();
        // TODO fix ledger library so can put in all ledgers
        if let Ok(raw_ledger_app) = RawLedgerApp::new() {
            let ledger_app = LedgerMasterCap::from_ledger(raw_ledger_app)?;
            self.discovered_devices
                .insert(ledger_app.id.clone(), ledger_app);
        }
        Ok(())
    }
}

impl AbstractKeyStore for LedgerKeyStore {
    const SOURCE_NAME: &'static str = "ledger hardware wallet";

    type Err = LedgerKeyStoreError;

    type AccountId = LedgerId;

    type AccountCap = LedgerMasterCap;

    fn list_accounts(&mut self) -> Result<Box<dyn Iterator<Item = Self::AccountId>>, Self::Err> {
        self.refresh()?;
        let key_copies: Vec<_> = self.discovered_devices.keys().cloned().collect();
        Ok(Box::new(key_copies.into_iter()))
    }

    fn from_dir(_dir: PathBuf, _scrypt_type: ScryptType) -> Result<Self, LedgerKeyStoreError> {
        // TODO maybe force the initialization of the HidAPI "lazy static"?
        Ok(LedgerKeyStore::new())
    }

    fn borrow_account<'a, 'b>(
        &'a mut self,
        account_id: &'b Self::AccountId,
    ) -> Result<&'a Self::AccountCap, Self::Err> {
        self.refresh()?;
        self.discovered_devices
            .get(account_id)
            .ok_or_else(|| LedgerKeyStoreError::LedgerNotFound {
                id: account_id.clone(),
            })
    }
}

/// A ledger device with the Nervos app.
#[derive(Clone)]
pub struct LedgerMasterCap {
    id: LedgerId,
    // TODO no Arc once we have "generic associated types" and can just borrow the device.
    ledger_app: Arc<RawLedgerApp>,
}

impl LedgerMasterCap {
    /// Create from a ledger device, checking that a proper version of the
    /// Nervos app is installed.
    fn from_ledger(ledger_app: RawLedgerApp) -> Result<Self, LedgerKeyStoreError> {
        let command = apdu::get_wallet_id();
        let response = ledger_app.exchange(command)?;
        debug!("Nervos CKB Ledger app wallet id: {:02x?}", response);

        let mut resp = &response.data[..];
        // TODO: The ledger app gives us 64 bytes but we only use 32
        // bytes. We should either limit how many the ledger app
        // gives, or take all 64 bytes here.
        let raw_wallet_id = parse::split_off_at(&mut resp, 32)?;
        let _ = parse::split_off_at(&mut resp, 32)?;
        parse::assert_nothing_left(resp)?;

        Ok(LedgerMasterCap {
            id: LedgerId(H256::from_slice(raw_wallet_id).unwrap()),
            ledger_app: Arc::new(ledger_app),
        })
    }
}

const WRITE_ERR_MSG: &'static str = "IO error not possible when writing to Vec last I checked";

impl AbstractMasterPrivKey for LedgerMasterCap {
    type Err = LedgerKeyStoreError;

    type Privkey = LedgerCap;

    fn extended_privkey(&self, path: &[ChildNumber]) -> Result<LedgerCap, Self::Err> {
        if !is_valid_derivation_path(path.as_ref()) {
            return Err(LedgerKeyStoreError::InvalidDerivationPath {
                path: path.as_ref().iter().cloned().collect(),
            });
        }

        Ok(LedgerCap {
            master: self.clone(),
            path: From::from(path.as_ref()),
        })
    }
}

/// A ledger device with the Nervos app constrained to a specific derivation path.
#[derive(Clone)]
pub struct LedgerCap {
    master: LedgerMasterCap,
    pub path: DerivationPath,
}

// Only not using impl trait because unstable
type LedgerClosure = Box<dyn FnOnce(Vec<u8>) -> Result<RecoverableSignature, LedgerKeyStoreError>>;

const MAX_APDU_SIZE: usize = 230;

bitflags::bitflags! {
    struct SignP1: u8 {
        // for the path
        const FIRST = 0b_0000_0000;
        // for the tx
        const NEXT  = 0b_0000_0001;
        //const HASH_ONLY_NEXT  = 0b_000_0010 | Self::NEXT.bits; // You only need it once
        const CHANGE_PATH = 0b_0001_0000;
        const IS_CONTEXT = 0b_0010_0000;
        const NO_FALLBACK = 0b_0100_0000;
        const LAST_MARKER = 0b_1000_0000;
        const MASK = Self::LAST_MARKER.bits | Self::NO_FALLBACK.bits | Self::IS_CONTEXT.bits;
    }
}

impl AbstractPrivKey for LedgerCap {
    type Err = LedgerKeyStoreError;

    type SignerSingleShot = SignEntireHelper<LedgerClosure>;

    fn public_key(&self) -> Result<secp256k1::PublicKey, Self::Err> {
        let mut data = Vec::new();
        data.write_u8(self.path.as_ref().len() as u8)
            .expect(WRITE_ERR_MSG);
        for &child_num in self.path.as_ref().iter() {
            data.write_u32::<BigEndian>(From::from(child_num))
                .expect(WRITE_ERR_MSG);
        }
        let command = apdu::extend_public_key(data);
        let response = self.master.ledger_app.exchange(command)?;
        debug!(
            "Nervos CBK Ledger app extended pub key raw public key {:02x?} for path {:?}",
            &response, &self.path
        );
        let mut resp = &response.data[..];
        let len = parse::split_first(&mut resp)? as usize;
        let raw_public_key = parse::split_off_at(&mut resp, len)?;
        parse::assert_nothing_left(resp)?;
        Ok(PublicKey::from_slice(&raw_public_key)?)
    }

    fn sign(&self, _message: &H256) -> Result<Signature, Self::Err> {
        unimplemented!("Need to generalize method to not take hash")
        //let signature = self.sign_recoverable(message)?;
        //Ok(RecoverableSignature::to_standard(&signature))
    }

    fn begin_sign_recoverable(&self) -> Self::SignerSingleShot {
        let my_self = self.clone();

        SignEntireHelper::new(Box::new(move |message: Vec<u8>| {
            debug!(
                "Sending Nervos CKB Ledger app message of {:02x?} with length {:?}",
                message,
                message.len()
            );

            // Need to fill in missing “path” from signer.
            let mut raw_path = Vec::<Uint32>::new();
            for &child_num in my_self.path.as_ref().iter() {
                let raw_child_num: u32 = child_num.into();
                let raw_path_bytes = raw_child_num.to_le_bytes();
                raw_path.push(
                    Uint32::new_builder()
                        .nth0(raw_path_bytes[0].into())
                        .nth1(raw_path_bytes[1].into())
                        .nth2(raw_path_bytes[2].into())
                        .nth3(raw_path_bytes[3].into())
                        .build(),
                )
            }

            let message_with_sign_path = AnnotatedTransaction::from_slice(&message).unwrap();
            let sign_path = Bip32::new_builder().set(raw_path).build();
            let change_path = if message_with_sign_path.change_path().len() == 0 {
                sign_path.clone()
            } else {
                message_with_sign_path.change_path()
            };

            let raw_message = message_with_sign_path
                .as_builder()
                .sign_path(sign_path)
                .change_path(change_path)
                .build();

            debug!(
                "Modified Nervos CKB Ledger app message of {:02x?} with length {:?}",
                raw_message.as_slice(),
                raw_message.as_slice().len()
            );

            let chunk = |mut message: &[u8]| -> Result<_, Self::Err> {
                assert!(message.len() > 0, "initial message must be non-empty");
                let mut base = SignP1::FIRST;
                loop {
                    let length = ::std::cmp::min(message.len(), MAX_APDU_SIZE);
                    let chunk = parse::split_off_at(&mut message, length)?;
                    let rest_length = message.len();
                    let response = my_self.master.ledger_app.exchange(ApduCommand {
                        cla: 0x80,
                        ins: 0x03,
                        p1: (if rest_length > 0 {
                            base
                        } else {
                            base | SignP1::LAST_MARKER
                        })
                        .bits,
                        p2: 0,
                        length: chunk.len() as u8,
                        data: chunk.to_vec(),
                    })?;
                    if rest_length == 0 {
                        return Ok(response);
                    }
                    base = SignP1::NEXT;
                }
            };

            let response = chunk(raw_message.as_slice().as_ref())?;

            debug!(
                "Received Nervos CKB Ledger result of {:02x?} with length {:?}",
                response.data,
                response.data.len()
            );

            let raw_signature = response.data.clone();
            let mut resp = &raw_signature[..];

            let data = parse::split_off_at(&mut resp, 64)?;
            let recovery_id = RecoveryId::from_i32(parse::split_first(&mut resp)? as i32)?;
            debug!("Recovery id is {:?}", recovery_id);
            parse::assert_nothing_left(resp)?;

            Ok(RecoverableSignature::from_compact(data, recovery_id)?)
        }))
    }
}
