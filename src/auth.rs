use anyhow::Result;

use crate::error::Error;

pub trait Authenticator: Send + Sync {
    fn auth_needed(&self) -> bool;
    fn authenticate(&self, api_key: String) -> Result<()>;
    // fn clone(&self) -> Self where Self: Sized;
}

pub struct SingleKeyAuthenticator {
    api_key: String,
}

impl SingleKeyAuthenticator {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

impl Authenticator for SingleKeyAuthenticator {
    fn auth_needed(&self) -> bool {
        return &self.api_key != "";
    }

    fn authenticate(&self, api_key: String) -> Result<()> {
        if api_key != self.api_key {
            return Err(Error::Unauthorized.into());
        }

        return Ok(());
    }
}
