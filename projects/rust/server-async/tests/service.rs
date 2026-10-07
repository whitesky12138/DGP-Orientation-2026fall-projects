use rm_server_async::Service;
use serde_json::{Value, json};
use std::time::Duration;

#[test]
fn input_validation_and_baseline() {
    let service = Service::default();
    assert_eq!(
        service.handle("GET", "/ping", &Value::Null, ""),
        (200, json!({"data":"pong"}))
    );
    for body in [
        Value::Null,
        json!([]),
        json!({"username":true,"password":"password1"}),
        json!({"username":"a/b","password":"password1"}),
    ] {
        assert_eq!(service.handle("POST", "/users", &body, "").0, 400);
    }
    assert_eq!(service.handle("GET", "/texts", &Value::Null, "").0, 401);
    assert_eq!(service.handle("GET", "/missing", &Value::Null, "").0, 404);
}

#[test]
fn concurrent_registration_has_one_winner() {
    let service = std::sync::Arc::new(Service::default());
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let service = service.clone();
            std::thread::spawn(move || {
                service
                    .handle(
                        "POST",
                        "/users",
                        &json!({"username":"alice","password":"password1"}),
                        "",
                    )
                    .0
            })
        })
        .collect();
    let statuses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(statuses.iter().filter(|&&s| s == 201).count(), 1);
    assert_eq!(statuses.iter().filter(|&&s| s == 409).count(), 3);
}

#[test]
fn echo_and_text_lifecycle() {
    let service = Service::default();
    let account = json!({"username":"alice", "password":"password1"});

    assert_eq!(service.handle("POST", "/users", &account, "").0, 201);

    let login = service.handle("POST", "/sessions", &account, "").1;
    let token = format!("Bearer {}", login["data"]["token"].as_str().unwrap());
    //echo_test
    assert_eq!(
        service.handle("POST", "/echo", &json!({"text":"hello\nRM"}), ""),
        (200, json!({"data":"hello\nRM"}))
    );
    //text put/get test
    assert_eq!(
        service
            .handle("PUT", "/texts/note", &json!({"text":"first"}), &token,)
            .0,
        200
    );
    assert_eq!(
        service.handle("GET", "/texts/note", &Value::Null, &token),
        (200, json!({"data":"first"}))
    );
    //cover test
    assert_eq!(
        service
            .handle("PUT", "/texts/note", &json!({"text":"second"}), &token,)
            .0,
        200
    );
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &token),
        (200, json!({"data":["note"]}))
    );
    assert_eq!(
        service.handle("GET", "/texts/note", &Value::Null, &token),
        (200, json!({"data":"second"}))
    );
    //delete test
    assert_eq!(
        service.handle("DELETE", "/texts/note", &Value::Null, &token),
        (200, json!({"data":null}))
    );
    assert_eq!(
        service.handle("GET", "/texts/note", &Value::Null, &token).0,
        404
    );
}

#[test]
fn users_are_isolated_and_logout_deletes_data() {
    let service = Service::default();
    let alice = json!({"username":"alice", "password":"password1"});
    let bob = json!({"username":"bob", "password":"password1"});

    assert_eq!(service.handle("POST", "/users", &alice, "").0, 201);
    assert_eq!(service.handle("POST", "/users", &bob, "").0, 201);

    let alice_login = service.handle("POST", "/sessions", &alice, "").1;
    let alice_token = format!("Bearer {}", alice_login["data"]["token"].as_str().unwrap());
    let bob_login = service.handle("POST", "/sessions", &bob, "").1;
    let bob_token = format!("Bearer {}", bob_login["data"]["token"].as_str().unwrap());
    
    //Confirm that the two users read different content
    assert_eq!(
        service
            .handle(
                "PUT",
                "/texts/note",
                &json!({"text":"alice text"}),
                &alice_token,
            )
            .0,
        200
    );
    assert_eq!(
        service
            .handle(
                "PUT",
                "/texts/note",
                &json!({"text":"bob text"}),
                &bob_token,
            )
            .0,
        200
    );
    assert_eq!(
        service.handle("GET", "/texts/note", &Value::Null, &alice_token),
        (200, json!({"data":"alice text"}))
    );
    assert_eq!(
        service.handle("GET", "/texts/note", &Value::Null, &bob_token),
        (200, json!({"data":"bob text"}))
    );
    //delete test
    assert_eq!(
        service.handle("DELETE", "/users/me", &Value::Null, &alice_token),
        (200, json!({"data":null}))
    );
    //access with old token after logout
    assert_eq!(
        service
            .handle("GET", "/texts", &Value::Null, &alice_token)
            .0,
        401
    );
    //re-register with the same name
    assert_eq!(service.handle("POST", "/users", &alice, "").0, 201);
    let new_login = service.handle("POST", "/sessions", &alice, "").1;
    let new_token = format!("Bearer {}", new_login["data"]["token"].as_str().unwrap());
    assert_eq!(
        service.handle("GET", "/texts", &Value::Null, &new_token),
        (200, json!({"data":[]}))
    );
}

