use failure::Fail;

use super::Bip32Error;
use super::KeyStoreError;

#[derive(Debug, Fail)]
pub enum Error_<AbsKeyStoreErr: Fail> {
    #[fail(display = "BIP32 error: {}", _0)]
    BIP32(Bip32Error),

    #[fail(display = "KeyStore error: {}", _0)]
    KeyStore(AbsKeyStoreErr),

    #[fail(display = "Other error: {}", _0)]
    Other(String),
}

pub type Error = Error_<KeyStoreError>;

impl From<String> for Error {
    fn from(err: String) -> Error {
        Error::Other(err)
    }
}

impl From<&str> for Error {
    fn from(err: &str) -> Error {
        Error::Other(err.to_owned())
    }
}
