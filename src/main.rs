#![feature(hash_set_entry)]

// internal defs
mod auth;
mod ingress;
mod user;

#[macro_use]
extern crate serde;

extern crate actix_web;

use actix::System;
use actix_files as fs;
use actix_web::http::StatusCode;
use actix_web::web::{self, Path};
use actix_web::{get, App, HttpRequest, HttpResponse, HttpServer, Result};
use tokio::runtime::Runtime;
use tokio::task;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use ingress::RpcConnection;
use std::{iter::Map, thread};

const WALLET_ADDRESS: &'static str = env!("WALLET_ADDRESS");

/// Gets the website index.
async fn index(_req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(include_str!("../static/index.html")))
}

/// Gets the register page info.
#[get("/register.html")]
async fn register(_req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(include_str!("../static/register.html")))
}

#[get("/login.html")]
async fn login(_req: HttpRequest) -> Result<HttpResponse> {
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(include_str!("../static/login.html")))
}

/// Gets any assets served by the website.
#[get("/assets/{tail:.*}")]
async fn serve_asset(path: Path<(String,)>) -> Result<fs::NamedFile> {
    Ok(fs::NamedFile::open(format!("static/assets/{}", path.0 .0))?)
}

/// Loads the site's style
#[get("/style.css")]
async fn style(_req: HttpRequest) -> Result<fs::NamedFile> {
    Ok(fs::NamedFile::open("static/style.css")?)
}

fn main() -> std::io::Result<()> {
    let db = sled::open("db").unwrap();

    thread::spawn(|| {
        let rt = Runtime::new().unwrap();
        let task = task::LocalSet::new();

        rt.block_on(task.run_until(async move {
            let rpc = RpcConnection::new("http://root:none@0.0.0.0:8332/");

            // Every last day of the month, process all bitcoin payments
            loop {
                let today: DateTime<Utc> = Utc::now();
                let last_day_month = Utc
                    .ymd(
                        today.year(),
                        today.month(),
                        match today.month() {
                            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                            2 => 28,
                            _ => 30,
                        },
                    )
                    .and_hms(0, 0, 0);

                thread::sleep((last_day_month - today).to_std().unwrap());

                // Synchronously get an iterator over all of the existing utxo's with a non-nil
                // balance
                let all_utxos: Map<_, _> = rpc
                    .get_all_addresses()
                    .await
                    .unwrap()
                    .map(|addr| rpc.get_all_utxos(addr.to_owned()));

                // Send a transaction with ma MONEY
                rpc.reduce_utxos(WALLET_ADDRESS, all_utxos).await.unwrap();
            }
        }));
    });

    System::run(|| {
        HttpServer::new(move || {
            App::new()
                .data(db.clone())
                .data(RpcConnection::new("http://root:none@0.0.0.0:8332/"))
                .service(style)
                .service(serve_asset)
                .service(register)
                .service(login)
                .service(auth::register)
                .service(auth::login)
                .service(auth::new_post)
                .service(auth::new_wallet)
                .service(auth::account_overview)
                .service(auth::load_post)
                .service(auth::load_feed_page)
                .route("/index.html", web::get().to(index))
                .route("/", web::get().to(index))
        })
        .bind("0.0.0.0:7777")
        .unwrap()
        .run();
    })
}
