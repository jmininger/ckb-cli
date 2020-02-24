use std::path::PathBuf;

use ::std::fmt::Debug;

use log::debug;

use byteorder::{BigEndian, WriteBytesExt};
use ckb_sdk::wallet::{
    AbstractKeyStore, AbstractMasterPrivKey, ChildNumber, ExtendedPubKey, ScryptType,
};
use ckb_types::H160;

use secp256k1::key::PublicKey;

use ledger::{LedgerApp, LedgerError};

pub mod apdu;
mod error;

use error::Error as LedgerKeyStoreError;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}

pub struct LedgerKeyStore {
    ledger_app: Option<LedgerApp>,
}

impl LedgerKeyStore {
    fn init(&mut self) -> Result<&mut LedgerApp, LedgerError> {
        match self.ledger_app {
            Some(ref mut ledger_app) => Ok(ledger_app),
            None => {
                self.ledger_app = Some(LedgerApp::new()?);
                self.init()
            }
        }
    }

    fn check_version(&mut self) -> Result<(), LedgerError> {
        let ledger_app = self.init()?;
        {
            let command = apdu::app_version();
            let response = ledger_app.exchange(command)?;;
            debug!("Nervos CBK Ledger app Version: {:?}", response);
        }
        {
            let command = apdu::app_git_hash();
            let response = ledger_app.exchange(command)?;
            debug!("Nervos CBK Ledger app Git Hash: {:?}", response);
        }
        Ok(())
    }
}

impl AbstractKeyStore for LedgerKeyStore {
    const SOURCE_NAME: &'static str = "ledger hardware wallet";

    type Err = LedgerKeyStoreError;

    fn list_accounts(&mut self) -> Result<Box<dyn Iterator<Item = (usize, H160)>>, Self::Err> {
        let _ = self.check_version(); //.expect("oh no!");
        Ok(Box::new(::std::iter::empty()))
    }

    fn from_dir(_dir: PathBuf, _scrypt_type: ScryptType) -> Result<Self, LedgerKeyStoreError> {
        //unimplemented!()
        Ok(LedgerKeyStore { ledger_app: None })
    }
}

impl AbstractMasterPrivKey for &mut LedgerKeyStore {
    type Err = LedgerKeyStoreError;

    fn extended_pubkey<P>(self, path: &P) -> Result<ExtendedPubKey, Self::Err>
    where
        P: ?Sized + Debug + AsRef<[ChildNumber]>,
    {
        let ledger_app = self.init()?;
        let mut data = Vec::new();
        for &child_num in path.as_ref().iter() {
            data.write_u32::<BigEndian>(From::from(child_num))
                .expect("IO error not possible when writing to Vec last I checked");
        }
        let command = apdu::extend_public_key(data);
        let response = ledger_app.exchange(command)?;
        debug!(
            "Nervos CBK Ledger app extended pub key raw {:?}",
            (path, response)
        );
        Ok(ExtendedPubKey {
            depth: path.as_ref().len() as u8,
            parent_fingerprint: unimplemented!(),
            child_number: ChildNumber::from_hardened_idx(0)?,
            public_key: PublicKey::from_slice(&response.data)?,
            chain_code: unimplemented!(),
        })
    }
}