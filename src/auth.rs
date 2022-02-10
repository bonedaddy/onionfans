use super::{
    ingress::RpcConnection,
    user::{User, MONTHLY_BTC},
};

use actix_files as afs;
use actix_web::{
    cookie::Cookie,
    error, get, http,
    http::StatusCode,
    post,
    web::{self, Json, Path},
    Error as ActixError, HttpMessage, HttpRequest, HttpResponse, Responder, Result,
};
use askama::Template;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use sled::Db;

use std::{
    fs,
    time::{Duration, SystemTime},
};

const ITEMS_PER_PAGE: usize = 5;
const ITEM_PATH: &'static str = env!("CONTENT_FOLDER");
const ADMIN_PASS: &'static str = env!("ADMIN_PASS");

#[derive(Deserialize, Debug)]
pub struct Registration {
    username: String,
    password: String,
}

#[derive(Template)]
#[template(path = "account_overview.html")]
struct OverviewTemplate<'a> {
    username: &'a str,
    balance: f64,
    account_wallets: Vec<String>,
    balance_insufficient: bool,
}

#[derive(Template)]
#[template(path = "feed.html")]
struct FeedTemplate {
    posts: Vec<Post>,
    i: usize,
}

#[derive(Serialize, Deserialize)]
pub struct PostHist {
    path: String,
    created_at: SystemTime,
}

pub struct Post {
    isvideo: bool,
    caption: String,
    src: String,
}

#[derive(Deserialize)]
pub struct PostRq {
    caption: String,
    src: String,
    password: String,
}

macro_rules! auth_user {
    ($u:ident, $pass:expr) => {
        // Make sure the user is who they say they are
        if !argon2::verify_encoded(&$u.password_hash, $pass.as_bytes())
            .map_err(|e| error::ErrorInternalServerError(e))?
        {
            return Err(error::ErrorForbidden(
                "invalid username/password combination",
            ));
        }
    };
}

/// Load an individual feed page.
#[get("/feed/{i}.html")]
pub async fn load_feed_page(
    db_arc: web::Data<Db>,
    info: Path<usize>,
    req: HttpRequest,
) -> Result<HttpResponse, ActixError> {
    let mut db = (**db_arc).clone();

    let username = req
        .cookie("username")
        .ok_or(error::ErrorBadRequest("no username".to_owned()))?
        .to_string();
    let username_bytes: Vec<u8> =
        bincode::serialize::<str>(username.split("=").skip(1).collect::<Vec<&str>>()[0])
            .map_err(|e| error::ErrorInternalServerError(e.to_string()))?;

    // Get the entry for the user indicated by the username cookie
    let u: User = db
        .get(username_bytes)
        .map_err(|e| error::ErrorInternalServerError(e.to_string()))?
        .ok_or(error::ErrorInternalServerError(
            "user doesn't exist".to_owned(),
        ))
        .and_then(|u_bytes| {
            bincode::deserialize(u_bytes.to_vec().as_slice())
                .map_err(|e| error::ErrorInternalServerError(e.to_string()))
        })?;

    auth_user!(
        u,
        req.cookie("password")
            .ok_or(error::ErrorInternalServerError("no password"))?
            .to_string()
            .split("=")
            .skip(1)
            .collect::<Vec<&str>>()[0]
    );

    load_feed(&mut db, info.0).await.map(|resp| {
        HttpResponse::build(StatusCode::OK)
            .content_type("text/html; charset=utf-8")
            .body(resp)
    })
}

/// Registers a new post
#[post("/new_post")]
pub async fn new_post(
    db_arc: web::Data<Db>,
    json_info: Json<PostRq>,
) -> Result<HttpResponse, ActixError> {
    if json_info.password != ADMIN_PASS {
        return Err(error::ErrorUnauthorized("no password provided"));
    }

    let db = (**db_arc).clone();
    db.insert(
        bincode::serialize(&json_info.src).map_err(|e| error::ErrorInternalServerError(e))?,
        bincode::serialize(&json_info.caption).map_err(|e| error::ErrorInternalServerError(e))?,
    )
    .map_err(|e| error::ErrorInternalServerError(e))?;

    Ok(HttpResponse::Ok().content_type("plain/text").body("yay"))
}

/// Logs the user in the database and loads their feed.
#[post("/login")]
pub async fn login(
    db_arc: web::Data<Db>,
    btcapi: web::Data<RpcConnection<'_>>,
    form: web::Form<Registration>,
) -> impl Responder {
    let form_data = form.into_inner();
    let mut db = (**db_arc).clone();

    // Load the user from the database so we can verify that their password is correct
    let u: User = db
        .get(
            bincode::serialize(&form_data.username)
                .map_err(|e| error::ErrorInternalServerError(e))?,
        )
        .map_err(|e| error::ErrorInternalServerError(e))?
        .ok_or(error::ErrorInternalServerError("user not found"))
        .and_then(|user_bytes| {
            bincode::deserialize(&user_bytes).map_err(|e| error::ErrorInternalServerError(e))
        })?;

    auth_user!(u, form_data.password);

    (if u
        .get_account_balance(&**btcapi)
        .await
        .map_err(|e| error::ErrorPaymentRequired(e))?
        >= MONTHLY_BTC
        || u.username == "admin"
    {
        // Show the user the feed
        load_feed(&mut db, 0).await
    } else {
        load_account_overview(u, &**btcapi).await
    })
    .map(|res| {
        let mut resp = HttpResponse::build(StatusCode::OK)
            .content_type("text/html; charset=utf-8")
            .body(res);

        resp.add_cookie(&Cookie::new("username", form_data.username))
            .unwrap();
        resp.add_cookie(&Cookie::new("password", form_data.password))
            .unwrap();

        resp
    })
}

/// Registers the user in the database and loads the feed.
#[post("/register")]
pub async fn register(
    db_arc: web::Data<Db>,
    btcapi: web::Data<RpcConnection<'_>>,
    form: web::Form<Registration>,
) -> impl Responder {
    // Sled lets us clone db very cheaply, and a clone will reference the same underlying db.
    let mut db = (**db_arc).clone();
    let form_data = form.into_inner();
    let mut u = User::new(form_data.username.clone(), form_data.password.clone())
        .map_err(|e| e.to_string())
        .map_err(|e| error::ErrorInternalServerError(e))?;
    u.generate_new_acc_address(&**btcapi)
        .await
        .map_err(|e| error::ErrorInternalServerError(e))?;

    // The user is already signed up
    if db
        .contains_key(
            bincode::serialize(&u.username).map_err(|e| error::ErrorInternalServerError(e))?,
        )
        .map_err(|e| error::ErrorInternalServerError(e))?
    {
        return Err(error::ErrorUnauthorized("user already exists"));
    }

    u.commit(&mut db)
        .map_err(|e| error::ErrorInternalServerError(e))?;

    (if u
        .get_account_balance(&**btcapi)
        .await
        .map_err(|e| error::ErrorPaymentRequired(e))?
        >= MONTHLY_BTC
    {
        // Show the user the feed
        load_feed(&mut db, 0).await
    } else {
        load_account_overview(u, &**btcapi).await
    })
    .map(|str_html| {
        let mut resp = HttpResponse::build(StatusCode::OK)
            .header(http::header::LOCATION, "/account_overview.html")
            .content_type("text/html; charset=utf-8")
            .body(str_html);
        resp.add_cookie(&Cookie::new("username", form_data.username))
            .unwrap();
        resp.add_cookie(&Cookie::new("password", form_data.password))
            .unwrap();
        resp
    })
}

#[get("/account_overview.html")]
pub async fn account_overview(
    db_arc: web::Data<Db>,
    btcapi: web::Data<RpcConnection<'_>>,
    req: HttpRequest,
) -> Result<HttpResponse, ActixError> {
    let db = (**db_arc).clone();

    let username = req
        .cookie("username")
        .ok_or(error::ErrorBadRequest("no username".to_owned()))?
        .to_string();
    let username_bytes: Vec<u8> =
        bincode::serialize::<str>(username.split("=").skip(1).collect::<Vec<&str>>()[0])
            .map_err(|e| error::ErrorInternalServerError(e.to_string()))?;

    // Get the entry for the user indicated by the username cookie
    let u: User = db
        .get(username_bytes)
        .map_err(|e| error::ErrorInternalServerError(e.to_string()))?
        .ok_or(error::ErrorInternalServerError(
            "user doesn't exist".to_owned(),
        ))
        .and_then(|u_bytes| {
            bincode::deserialize(u_bytes.to_vec().as_slice())
                .map_err(|e| error::ErrorInternalServerError(e.to_string()))
        })?;

    auth_user!(
        u,
        req.cookie("password")
            .ok_or(error::ErrorInternalServerError("no password"))?
            .to_string()
            .split("=")
            .skip(1)
            .collect::<Vec<&str>>()[0]
    );

    // Show the user their page
    load_account_overview(u, &btcapi).await.map(|text_resp| {
        HttpResponse::build(StatusCode::OK)
            .content_type("text/html; charset=utf-8")
            .body(text_resp)
    })
}

#[get("/new_wallet")]
pub async fn new_wallet(
    db_arc: web::Data<Db>,
    btcapi: web::Data<RpcConnection<'_>>,
    req: HttpRequest,
) -> Result<HttpResponse, ActixError> {
    let mut db = (**db_arc).clone();

    let username = req
        .cookie("username")
        .ok_or(error::ErrorBadRequest("no username".to_owned()))?
        .to_string();
    let username_bytes: Vec<u8> =
        bincode::serialize::<str>(username.split("=").skip(1).collect::<Vec<&str>>()[0])
            .map_err(|e| error::ErrorInternalServerError(e.to_string()))?;

    // Get the entry for the user indicated by the username cookie
    let mut u: User = db
        .get(username_bytes)
        .map_err(|e| error::ErrorInternalServerError(e.to_string()))?
        .ok_or(error::ErrorInternalServerError(
            "user doesn't exist".to_owned(),
        ))
        .and_then(|u_bytes| {
            bincode::deserialize(u_bytes.to_vec().as_slice())
                .map_err(|e| error::ErrorInternalServerError(e.to_string()))
        })?;

    auth_user!(
        u,
        req.cookie("password")
            .ok_or(error::ErrorInternalServerError("no password"))?
            .to_string()
            .split("=")
            .skip(1)
            .collect::<Vec<&str>>()[0]
    );

    u.generate_new_acc_address(&**btcapi)
        .await
        .map_err(|e| error::ErrorInternalServerError(e))?;
    u.commit(&mut db)
        .map_err(|e| error::ErrorInternalServerError(e))?;

    // Show the user their page with the new wallet added
    load_account_overview(u, &btcapi).await.map(|text_resp| {
        HttpResponse::build(StatusCode::OK)
            .header(http::header::LOCATION, "/account_overview.html")
            .content_type("text/html; charset=utf-8")
            .body(text_resp)
    })
}

/// Loads an indvidiaul picture / video from a post.
#[get("/posts/{post_id}")]
pub async fn load_post(db_arc: web::Data<Db>, info: Path<String>) -> Result<afs::NamedFile> {
    // posts are referenced by their bincode UID reprs
    let post_uid =
        bincode::serialize(info.0.as_str()).map_err(|e| error::ErrorInternalServerError(e))?;
    let post_src: PostHist = bincode::deserialize(
        db_arc
            .get(&post_uid)
            .map_err(|e| error::ErrorInternalServerError(e))?
            .ok_or(error::ErrorNotFound("content does not exist"))?
            .to_vec()
            .as_slice(),
    )
    .map_err(|e| error::ErrorInternalServerError(e))?;

    if SystemTime::now() - Duration::new(60, 0) > post_src.created_at {
        db_arc
            .remove(post_uid)
            .map_err(|e| error::ErrorInternalServerError(e))?;

        return Err(error::ErrorNotFound("content not found"));
    }

    Ok(afs::NamedFile::open(post_src.path)?)
}

/// Responds to a user request with a oawefjoiawjfoiaewfjcustomized template of the feed.
/// i: the index of the content to start on
pub async fn load_feed(db: &mut Db, i: usize) -> Result<String> {
    let mut identifiers = Vec::new();
    let real_sources = fs::read_dir(ITEM_PATH).map_err(|e| error::ErrorInternalServerError(e))?;

    // Register single-use IDs for the content we want to serve to the user
    for j in i..(i + ITEMS_PER_PAGE) {
        // Store a mapping between the random identifier and the content to be served
        let id = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(9)
            .map(char::from)
            .collect();
        db.insert(
            bincode::serialize(&id).map_err(|e| error::ErrorInternalServerError(e))?,
            bincode::serialize(&j).map_err(|e| error::ErrorInternalServerError(e))?,
        )
        .map_err(|e| error::ErrorInternalServerError(e))?;
        identifiers.push(id);
    }

    FeedTemplate {
        i: i,
        posts: real_sources
            .filter_map(Result::ok)
            .enumerate()
            .filter(|(index, _source)| *index >= (i * ITEMS_PER_PAGE))
            .take(ITEMS_PER_PAGE)
            .zip(identifiers)
            .map(|((_i, source), mut uid): ((_, _), String)| {
                source
                    .path()
                    .to_str()
                    .ok_or(error::ErrorInternalServerError("malformed path"))
                    .and_then(|path_str| {
                        if path_str.contains("mov") {
                            uid += ".mov"
                        } else {
                            uid += ".jpg"
                        }

                        bincode::serialize(path_str)
                            .map_err(|e| error::ErrorInternalServerError(e))
                            .and_then(|ser_path| {
                                bincode::serialize(&uid)
                                    .map_err(|e| error::ErrorInternalServerError(e))
                                    .and_then(|uid_bytes| {
                                        bincode::serialize(&PostHist {
                                            path: path_str.to_owned(),
                                            created_at: SystemTime::now(),
                                        })
                                        .map_err(|e| error::ErrorInternalServerError(e))
                                        .and_then(
                                            |ser_post| {
                                                db.insert(uid_bytes, ser_post)
                                                    .map_err(|e| error::ErrorInternalServerError(e))
                                                    .map(|_| ser_path)
                                            },
                                        )
                                    })
                            })
                            .and_then(|ser_path| {
                                db.get(ser_path)
                                    .map_err(|e| error::ErrorInternalServerError(e))
                                    .and_then(|possible_hit| {
                                        possible_hit
                                            .ok_or(error::ErrorInternalServerError("no entry"))
                                    })
                            })
                            .and_then(|db_entry| {
                                bincode::deserialize(&db_entry)
                                    .map_err(|e| error::ErrorInternalServerError(e))
                            })
                            .map(|caption: String| Post {
                                isvideo: path_str.contains("mov"),
                                caption,
                                src: format!("/posts/{}", uid),
                            })
                            .map_err(|e| error::ErrorInternalServerError(e))
                    })
            })
            .collect::<Result<Vec<Post>>>()?,
    }
    .render()
    .map_err(|e| error::ErrorInternalServerError(e))
}

/// Loads the user's account overview.
pub async fn load_account_overview(u: User, adapter: &RpcConnection<'_>) -> Result<String> {
    let balance = u
        .get_account_balance(adapter)
        .await
        .map_err(|e| error::ErrorInternalServerError(e))?;

    // Instantiate the account overview template
    OverviewTemplate {
        username: &u.username,
        balance_insufficient: balance < MONTHLY_BTC,
        balance: balance,
        account_wallets: u.btc_addresses.into_iter().map(|addr| addr).collect(),
    }
    .render()
    .map_err(|e| error::ErrorInternalServerError(e))
}
