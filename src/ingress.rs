use actix_web::client::Client;
use futures::future::{join_all, Future};
use serde_json::Value;
use std::iter::Iterator;

/// Represents a connection to some bitcoind RPC listener.
#[derive(Clone)]
pub struct RpcConnection<'a> {
    upstream_url: &'a str,
    rpc_client: Client,
}

impl<'a> RpcConnection<'a> {
    pub fn new(upstream_url: &'a str) -> Self {
        Self {
            upstream_url,
            rpc_client: Client::new(),
        }
    }
}

impl RpcConnection<'_> {
    /// Gets an iterator over all of the unspent transaction outputsfor the given address.
    pub async fn get_all_utxos(
        &self,
        addr: String,
    ) -> Result<impl Iterator<Item = (String, u64, f64)>, String> {
        self.rpc_client
            .post(self.upstream_url)
            .basic_auth("root", Some("none"))
            .send_body(format!(
                r#"{{"jsonrpc": "1.0", "method": "listunspent", "params": [1, 9999999, ["{}"]]}}"#,
                addr
            ))
            .await
            .map_err(|e| e.to_string())?
            .body()
            .await
            .map_err(|e| e.to_string())
            .map(|raw_resp| raw_resp.to_vec())
            .and_then(|byte_resp| String::from_utf8(byte_resp).map_err(|e| e.to_string()))
            .and_then(|str_resp| {
                serde_json::from_str::<Value>(str_resp.as_str()).map_err(|e| e.to_string())
            })
            .map(|mut json| json["result"].take())
            .and_then(|results: Value| match results {
                Value::Array(items) => Ok(items),
                _ => Err("invalid response".to_owned()),
            })
            .map(|results| {
                results
                    .into_iter()
                    .filter(|tx| tx["amount"].as_f64().unwrap_or_default() > 0.00)
                    .map(|mut sufficient_tx| {
                        (
                            sufficient_tx["txid"]
                                .take()
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                            sufficient_tx["vout"].as_u64().unwrap(),
                            sufficient_tx["amount"].as_f64().unwrap_or_default(),
                        )
                    })
            })
    }

    /// Formulates a transaction to redeem a set of utxo's and spends them.
    pub async fn reduce_utxos(
        &self,
        destination: &str,
        utxos: impl Iterator<
            Item = impl Future<Output = Result<impl Iterator<Item = (String, u64, f64)>, String>>,
        >,
    ) -> Result<(), String> {
        let (val, addrs) = join_all(utxos)
            .await
            .into_iter()
            .filter_map(Result::ok)
            .flatten()
            .enumerate()
            .fold(
                (0.00 as f64, "".to_owned()),
                |(total, ids), (i, (id, vout, val))| {
                    (
                        total + val,
                        format!(
                            r#"{}{} {{"txid": "{}", "vout": {}}}"#,
                            ids,
                            if i > 0 { "," } else { "" },
                            id,
                            vout,
                        ),
                    )
                },
            );

        println!(
            r#"{{"jsonrpc": "1.0", "method": "createrawtransaction", "params": [[{}], [{{"{}": {:.8}}}]]}}"#,
            addrs, destination, val - 0.000075,
        );

        // Generate a collective transaction
        let tx_hex = self.rpc_client
            .post(self.upstream_url)
            .basic_auth("root", Some("none"))
            .send_body(
        format!(
            r#"{{"jsonrpc": "1.0", "method": "createrawtransaction", "params": [[{}], [{{"{}": {:.8}}}]]}}"#,
            addrs, destination, val - 0.000075,
        ))
            .await
            .map_err(|e| e.to_string())?
            .body()
            .await
            .map_err(|e| e.to_string())
            .map(|raw_resp| raw_resp.to_vec())
            .and_then(|byte_resp| String::from_utf8(byte_resp).map_err(|e| e.to_string()))
            .and_then(|str_resp| {
                serde_json::from_str::<Value>(str_resp.as_str()).map_err(|e| e.to_string())
            })
        .and_then(|mut json| {
            if let Value::String(e) = json["error"].take() {
                Err(e)
            } else if let Value::String(tx) = json["result"].take() {
                Ok(tx)
            } else {
                Err("no response".to_owned())
            }
        })?;

        // Sign the collective transaction
        let signed_tx = self.rpc_client.post(self.upstream_url).basic_auth("root", Some("none")).send_body(format!(r#"{{"jsonrpc": "1.0", "method": "signrawtransactionwithwallet", "params": ["{}"]}}"#, tx_hex)).await
            .map_err(|e| e.to_string())?
            .body()
            .await
            .map_err(|e| e.to_string())
            .map(|raw_resp| raw_resp.to_vec())
            .and_then(|byte_resp| String::from_utf8(byte_resp).map_err(|e| e.to_string()))
            .and_then(|str_resp| {
                serde_json::from_str::<Value>(str_resp.as_str()).map_err(|e| e.to_string())
            })
        .and_then(|mut json| {
            if let Value::String(e) = json["error"].take() {
                Err(e)
            } else if let Value::String(tx) = json["result"]["hex"].take() {
                Ok(tx)
            } else {
                Err("no response".to_owned())
            }
        })?;

        // Broadcast the collective transaction
        self.rpc_client
            .post(self.upstream_url)
            .basic_auth("root", Some("none"))
            .send_body(format!(
                r#"{{"jsonrpc": "1.0", "method": "sendrawtransaction", "params": ["{}"]}}"#,
                signed_tx
            ))
            .await
            .map_err(|e| e.to_string())?
            .body()
            .await
            .map_err(|e| e.to_string())
            .map(|raw_resp| raw_resp.to_vec())
            .and_then(|byte_resp| String::from_utf8(byte_resp).map_err(|e| e.to_string()))
            .and_then(|str_resp| {
                serde_json::from_str::<Value>(str_resp.as_str()).map_err(|e| e.to_string())
            })
            .and_then(|mut json| {
                if let Value::String(e) = json["error"].take() {
                    Err(e)
                } else {
                    println!("GIMME MA MONEY: {:?}", json);

                    Ok(())
                }
            })
    }

    pub async fn get_all_addresses(&self) -> Result<impl Iterator<Item = String>, String> {
        self.rpc_client
            .post(self.upstream_url)
            .basic_auth("root", Some("none"))
            .send_body(r#"{"jsonrpc": "1.0", "method": "getaddressesbylabel", "params": [""]}"#)
            .await
            .map_err(|e| e.to_string())?
            .body()
            .await
            .map_err(|e| e.to_string())
            .map(|raw_resp| raw_resp.to_vec())
            .and_then(|byte_resp| String::from_utf8(byte_resp).map_err(|e| e.to_string()))
            .and_then(|str_resp| {
                serde_json::from_str::<Value>(str_resp.as_str()).map_err(|e| e.to_string())
            })
            .map(|mut json| json["result"].take())
            .and_then(|results: Value| {
                if let Value::Object(obj) = results {
                    Ok(obj.keys().cloned().collect::<Vec<String>>())
                } else {
                    Err("invalid response".to_owned())
                }
            })
            .map(|results: Vec<String>| results.into_iter())
    }

    pub async fn get_new_address(&self) -> Result<String, String> {
        // Request getaddressesbylabela new address from Bitcoind, and then get the address in the response
        // once the response has been received
        self.rpc_client
            .post(self.upstream_url)
            .basic_auth("root", Some("none"))
            .send_body(r#"{"jsonrpc": "1.0", "method": "getnewaddress", "params": []}"#)
            .await
            .map_err(|e| e.to_string())?
            .body()
            .await
            .map_err(|e| e.to_string())
            .map(|raw_resp| raw_resp.to_vec())
            .and_then(|byte_resp| {
                String::from_utf8(byte_resp)
                    .map_err(|e| e.to_string())
                    .and_then(|resp| serde_json::from_str(resp.as_str()).map_err(|e| e.to_string()))
                    .and_then(|mut resp_json: Value| {
                        // Match a correct response, nothing, or an error.
                        // Don't do this in a functional way since that would require
                        // extra allocations.
                        if let Value::String(s) = resp_json["result"].take() {
                            Ok(s)
                        } else if let Value::String(s) = resp_json["error"].take() {
                            Err(s)
                        } else {
                            Err("nil response".to_owned())
                        }
                    })
            })
    }

    /// Gets the balance of the given address.
    pub async fn get_address_balance(&self, addr: &str) -> Result<f64, String> {
        // Get all UTXO's for the given address
        self.rpc_client
            .post(self.upstream_url)
            .basic_auth("root", Some("none"))
            .send_body(format!(
                r#"{{"jsonrpc": "1.0", "method": "listunspent", "params": [1, 9999999, ["{}"]]}}"#,
                addr
            ))
            .await
            .map_err(|e| e.to_string())?
            .body()
            // Check for any errors and stop execution if they exist
            .await
            .map_err(|e| e.to_string())
            // Actix returns responses as a fat-ass actix resp
            .map(|raw_resp| raw_resp.to_vec())
            // Get the response as a string, then get the JSON from it
            .and_then(|bytes_resp| String::from_utf8(bytes_resp).map_err(|e| e.to_string()))
            .and_then(|s| serde_json::from_str(s.as_str()).map_err(|e| e.to_string()))
            .and_then(|mut json: Value| {
                json["error"]
                    .take()
                    .as_str()
                    .map_or(Ok(json), |e| Err(e.to_owned()))
            })
            // No errors yet, so try to sum up the UTXO amounts
            .map(|mut json: Value| json["result"].take())
            .and_then(|json| match json {
                Value::Array(vals) => Ok(vals),
                _ => Err(format!(
                    "invalid bitcoind response type (expected array of utxo's) (got {:?})",
                    json
                )),
            })
            .map(|txs| {
                txs.iter()
                    .filter_map(|tx| tx["amount"].as_f64())
                    .fold(0.00, |acc: f64, tx_val| acc + tx_val)
            })
    }
}
