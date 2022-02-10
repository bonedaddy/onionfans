use super::ingress::RpcConnection;

use argon2::Error as CryptoError;
use futures::future::try_join_all;
use rand::random;
use sled::Db;
use std::collections::HashSet;

/// Approximately 0.002 BTC / year
pub const MONTHLY_BTC: f64 = 0.0002;

/// A user registered on ecp.
#[derive(Serialize, Deserialize)]
pub struct User {
    /// The primary record of the user
    pub username: String,

    pub btc_addresses: HashSet<String>,
    pub password_hash: String,
    pub salt: [u8; 16],
}

impl<'a> User {
    /// Creates a new user with a unique salted password hash.
    pub fn new(username: String, password: String) -> Result<Self, CryptoError> {
        let salt = random::<[u8; 16]>();
        argon2::hash_encoded(password.as_bytes(), &salt, &argon2::Config::default()).map(
            |password_hash| Self {
                username,
                btc_addresses: HashSet::new(),
                password_hash,
                salt,
            },
        )
    }

    /// Generates a new bitcoin address for the account and adds it to the current instance.
    pub async fn generate_new_acc_address(
        &mut self,
        adapter: &RpcConnection<'_>,
    ) -> Result<&str, String> {
        // Keep the address for later so we can check that the user owns it
        Ok(&**self
            .btc_addresses
            .get_or_insert(adapter.get_new_address().await?))
    }

    /// Calculates the collective balance of the user.
    pub async fn get_account_balance(&self, adapter: &RpcConnection<'_>) -> Result<f64, String> {
        // Get the balances of each address owned by the user individually, then add them up.
        // Fail if any one of the addresses cannot have a balance calculated
        Ok(try_join_all(
            self.btc_addresses
                .iter()
                .map(|acc| adapter.get_address_balance(acc.as_str())),
        )
        .await?
        .into_iter()
        .fold(0.00, |accum: f64, addr_bal: f64| accum + addr_bal))
    }

    /// Determines whether or not the user has paid for this month.
    pub async fn has_paid_for_month(&self, adapter: &RpcConnection<'_>) -> Result<bool, String> {
        self.get_account_balance(adapter)
            .await
            .map(|balance| balance >= MONTHLY_BTC)
    }

    /// Saves the user to the database.
    pub fn commit(&self, db: &mut Db) -> Result<(), String> {
        bincode::serialize(&self.username)
            .map_err(|e| e.to_string())
            .and_then(|ser_uname| {
                db.insert(
                    ser_uname,
                    bincode::serialize(self).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())
                .map(|_| ())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_rt::test]
    async fn test_generate_new_acc_address() {
        let addr = RpcConnection::new("http://root:none@192.168.1.173:8332/")
            .get_new_address()
            .await;

        assert!(addr.is_ok());
        assert!(addr.unwrap().len() >= 26);
    }

    #[actix_rt::test]
    async fn test_get_address_balance() {
        // A test address that will always have a balance of greater than 0.
        const TEST_ADDR: &'static str = "bc1qy85uz8sf4w3erc695qfggzaexzt3tm7nkkmprk";

        // Lib
        let adapter = RpcConnection::new("http://root:none@192.168.1.173:8332/");

        // Below balance is what the test account started with
        assert!(adapter.get_address_balance(TEST_ADDR).await.unwrap() > 0.00016927);
    }

    #[actix_rt::test]
    async fn test_has_paid_for_month() {
        // A test address that will always have a balance of greater than 0.
        const TEST_ADDR: &'static str = "bc1qy85uz8sf4w3erc695qfggzaexzt3tm7nkkmprk";

        // Lib
        let adapter = RpcConnection::new("http://root:none@192.168.1.173:8332/");
        let acc = User {
            username: "lol".to_owned(),
            btc_addresses: [TEST_ADDR].iter().map(|s| s.to_string()).collect(),
            password_hash: "waejaweof".to_owned(),
            salt: [0; 16],
        };

        // Below balance is what the test account started with
        assert!(acc.has_paid_for_month(&adapter).await.unwrap());
    }
}
