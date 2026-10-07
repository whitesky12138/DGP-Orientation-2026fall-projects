pub mod http;

use pbkdf2::pbkdf2_hmac;
use rand::{RngCore, rngs::OsRng};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/ping"),
    ("POST", "/users"),
    ("POST", "/sessions"),
    ("POST", "/echo"),
    ("PUT", "/texts/{name}"),
    ("DELETE", "/sessions/current"),
    ("DELETE", "/texts/{name}"),
    ("DELETE", "/users/me"),
    ("GET", "/texts"),
    ("GET", "/texts/{name}"),
];

//text name extraction
fn text_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/texts/")?;

    if name.contains('/') {
        //invalid"/"
        None
    } else {
        Some(name)
    }
}

pub fn route_error(method: &str, path: &str) -> Option<u16> {
    // dynamic text path
    if text_name(path).is_some() {
        return if matches!(method, "PUT" | "GET" | "DELETE") {
            // /texts/{name} support PUT、GET、DELETE
            None
        } else {
            // path exists,but HTTP method is not support
            Some(405)
        };
    }
    // fixed path
    match ROUTES.iter().find(|(_, route)| *route == path) {
        None => Some(404),
        Some((allowed, _)) if *allowed != method => Some(405),
        Some(_) => None,
    }
}

//save content and period at the same time
struct Session {
    value: String,
    expires_at: Instant,
}
pub struct User {
    id:u64,
    pub salt: [u8; 16],
    pub digest: [u8; 32],
    token: Option<Session>,
    pub texts: BTreeMap<String, String>,
}
//manually implement default
impl Default for Service {
    fn default() -> Self {
        Self::with_token_ttl_seconds(300)
    }
}

impl Service {
    pub fn with_token_ttl_seconds(seconds: u64) -> Self {
        assert!(seconds > 0, "token TTL must be positive"); //validity period

        Self {
            users: Mutex::new(BTreeMap::new()),
            token_ttl: Duration::from_secs(seconds),
            next_user_id: AtomicU64::new(1),
        }
    }
}
pub struct Service {
    pub users: Mutex<BTreeMap<String, User>>,
    token_ttl: Duration,     //validity period
    next_user_id: AtomicU64, //assign unique ID to new user to distinguish accounts at different life cycle stages
}

pub fn error(status: u16, message: &str) -> (u16, Value) {
    (status, json!({"message": message}))
}

pub fn valid_name(name: &str, max: usize) -> bool {
    !name.is_empty()
        && name.len() <= max
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn password_hash(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut output = [0; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, 100_000, &mut output);
    output
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Service {
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        body: &Value,
        authorization: &str,
    ) -> (u16, Value) {
        if let Some(status) = route_error(method, path) {
            return error(
                status,
                if status == 404 {
                    "Not found"
                } else {
                    "Method not allowed"
                },
            );
        }
        if method == "GET" && path == "/ping" {
            return (200, json!({"data": "pong"}));
        }

        if method == "POST" && path == "/echo" {
            let Some(object) = body.as_object() else {
                //verify request body is a JSON object
                return error(400, "Expected object");
            };
            if object.len() != 1 {
                //confirm only one field
                return error(400, "Expected only text");
            }
            let Some(text) = object //Get the text field
                .get("text")
                .and_then(Value::as_str)
            else {
                return error(400, "Expected text");
            };
            if text.len() > 65_536 {
                //text length constrain
                return error(413, "Text too large");
            }
            return (200, json!({"data": text}));
        }

        if method == "POST" && matches!(path, "/users" | "/sessions") {
            let Some(name) = body.get("username").and_then(Value::as_str) else {
                return error(400, "Expected username");
            };
            let Some(password) = body.get("password").and_then(Value::as_str) else {
                return error(400, "Expected password");
            };
            if body.as_object().map(|v| v.len()) != Some(2)
                || !valid_name(name, 32)
                || !(8..=128).contains(&password.chars().count())
            {
                return error(400, "Invalid account fields");
            }
            if path == "/users" {
                let mut salt = [0; 16];
                OsRng.fill_bytes(&mut salt);
                let digest = password_hash(password, &salt);
                let mut users = self.users.lock().unwrap();
                if users.contains_key(name) {
                    return error(409, "Username exists");
                }

                let id = self.next_user_id.fetch_add(1, Ordering::Relaxed);

                users.insert(
                    name.into(),
                    User {
                        id,
                        salt,
                        digest,
                        token: None,
                        texts: BTreeMap::new(),
                    },
                );
                return (201, json!({"data": {"username": name}}));
            }

            let (user_id,salt, expected) = {
                let users = self.users.lock().unwrap();
                let Some(user) = users.get(name) else {
                    return error(401, "Invalid username or password");
                };
                (user.id,user.salt, user.digest)
            };
            let digest = password_hash(password, &salt);

            let mut users = self.users.lock().unwrap();
            let Some(user) = users.get_mut(name) else {
                return error(401, "Invalid username or password");
            };
            if user.salt != salt || user.id != user_id || !bool::from(digest.ct_eq(&expected)) {
                return error(401, "Invalid username or password");
            }

            let token = new_token();
            let expires_at = Instant::now() + self.token_ttl; //count expiaration time of the token
            let expires_in = self.token_ttl.as_secs(); //convert to integer seconds

            user.token = Some(Session {
                value: token.clone(),
                expires_at,
            });

            return (
                200,
                json!({
                    "data": {"token": token,"expires_in": expires_in}
                }),
            );
        }

        //login verification
        let protected =
            path == "/texts"
                || path == "/sessions/current"
                || path == "/users/me"
                || text_name(path).is_some();

        if protected {
            let token = authorization.strip_prefix("Bearer ").unwrap_or("");
            let mut users = self.users.lock().unwrap();
            
            let now = Instant::now(); //record current time
            let name = users
                .iter()
                .find(|(_, user)| {!token.is_empty() && user.token.as_ref().is_some_and(|session| {
                                                session.value == token  //token values are the same
                                                && now < session.expires_at //token not expired
                                            })
                })
                .map(|(name, _)| name.clone());
            let Some(name) = name else {
                return error(401, "Login required");
            };

            if method == "DELETE" && path == "/users/me" {
                users.remove(&name);

                return (200, json!({"data": null}));
            }

            let user = users.get_mut(&name).unwrap();
           
            // implement PUT /texts/{name}
            if let Some(text_name) = text_name(path){
                if !valid_name(text_name, 64) {  //check text name
                    return error(400, "Invalid text name");
                }
                if method == "PUT" {
                    let Some(object) = body.as_object() else { //body must be JSON object
                        return error(400, "Expected object");
                    };
                    if object.len() != 1 {  //only one field
                        return error(400, "Expected only text");
                    }
                    //get text field
                    let Some(text) = object
                        .get("text")
                        .and_then(Value::as_str)
                    else {
                        return error(400, "Expected text");
                    };
                    //check text length
                    if text.len() > 65_536 {
                        return error(413, "Text too large");
                    }
                    //save text
                    user.texts.insert(
                        text_name.to_owned(),
                        text.to_owned(),
                    );
                    return (200, json!({"data": null}));
                }

                if method == "GET" {
                    let Some(text) = user.texts.get(text_name) else {
                        return error(404, "Text not found");
                };
                return (200, json!({"data": text}));
                }
                
                if method == "DELETE" {
                    if user.texts.remove(text_name).is_none() {
                        return error(404, "Text not found");
                    }
                    return (200, json!({"data": null}));
                }
            }

            if method == "DELETE" && path == "/sessions/current" {
                user.token = None;
                return (200, json!({"data": null}));
            }
            if method == "GET" && path == "/texts" {
                return (200, json!({"data": user.texts.keys().collect::<Vec<_>>()}));
            }
        }
        error(404, "Not found")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_lifecycle() {
        let service = Service::default();
        let account = json!({"username":"alice", "password":"password1"});
        assert_eq!(service.handle("POST", "/users", &account, "").0, 201);
        assert_eq!(service.handle("POST", "/users", &account, "").0, 409);
        let login = service.handle("POST", "/sessions", &account, "").1;
        let old = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
        let login = service.handle("POST", "/sessions", &account, "").1;
        let current = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
        assert_ne!(old, current);
        assert_eq!(service.handle("GET", "/texts", &Value::Null, &old).0, 401);
        assert_eq!(
            service.handle("GET", "/texts", &Value::Null, &current),
            (200, json!({"data":[]}))
        );
        assert_eq!(
            service
                .handle("DELETE", "/sessions/current", &Value::Null, &current)
                .0,
            200
        );
        assert_eq!(
            service.handle("GET", "/texts", &Value::Null, &current).0,
            401
        );
    }
}
