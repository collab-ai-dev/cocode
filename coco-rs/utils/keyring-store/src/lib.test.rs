use super::CredentialStoreError;
use super::KeyringStore;
use keyring::Error as KeyringError;
use keyring::credential::CredentialApi as _;
use keyring::mock::MockCredential;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

#[derive(Default, Clone, Debug)]
pub struct MockKeyringStore {
    credentials: Arc<Mutex<HashMap<String, Arc<MockCredential>>>>,
}

impl MockKeyringStore {
    pub fn credential(&self, account: &str) -> Arc<MockCredential> {
        let mut guard = self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .entry(account.to_string())
            .or_insert_with(|| Arc::new(MockCredential::default()))
            .clone()
    }

    pub fn saved_value(&self, account: &str) -> Option<String> {
        let credential = {
            let guard = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.get(account).cloned()
        }?;
        credential.get_password().ok()
    }

    pub fn set_error(&self, account: &str, error: KeyringError) {
        let credential = self.credential(account);
        credential.set_error(error);
    }

    pub fn contains(&self, account: &str) -> bool {
        let guard = self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.contains_key(account)
    }
}

impl KeyringStore for MockKeyringStore {
    fn load(&self, _service: &str, account: &str) -> Result<Option<String>, CredentialStoreError> {
        let credential = {
            let guard = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.get(account).cloned()
        };

        let Some(credential) = credential else {
            return Ok(None);
        };

        match credential.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(CredentialStoreError::new(error)),
        }
    }

    fn save(&self, _service: &str, account: &str, value: &str) -> Result<(), CredentialStoreError> {
        let credential = self.credential(account);
        credential
            .set_password(value)
            .map_err(CredentialStoreError::new)
    }

    fn delete(&self, _service: &str, account: &str) -> Result<bool, CredentialStoreError> {
        let credential = {
            let guard = self
                .credentials
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            guard.get(account).cloned()
        };

        let Some(credential) = credential else {
            return Ok(false);
        };

        let removed = match credential.delete_credential() {
            Ok(()) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(CredentialStoreError::new(error)),
        }?;

        let mut guard = self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.remove(account);
        Ok(removed)
    }
}
