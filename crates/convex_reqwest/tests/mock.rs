//! Native tests through `convex_fn_test`'s mock host.

use std::{cell::RefCell, rc::Rc};

use convex_fn_test::{FetchStubRequest, FetchStubResponse, TestHost};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

type Seen = Rc<RefCell<Vec<FetchStubRequest>>>;

fn host(resp: impl Fn(&FetchStubRequest) -> FetchStubResponse + 'static) -> (TestHost, Seen) {
    let seen: Seen = Rc::default();
    let s = seen.clone();
    let host = TestHost::new().on_fetch(move |r| {
        s.borrow_mut().push(r.clone());
        resp(r)
    });
    (host, seen)
}

fn header<'a>(r: &'a FetchStubRequest, name: &str) -> Option<&'a str> {
    r.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Item {
    id: u32,
    name: String,
}

#[test]
fn get_text_status_headers_url() {
    let (h, seen) = host(|_| FetchStubResponse::new(200, "hello").header("x-test", "1"));
    h.run(|_| async {
        let client = reqwest::Client::builder()
            .user_agent("convex-test/1")
            .default_headers({
                let mut m = reqwest::header::HeaderMap::new();
                m.insert("x-default", "d".parse().unwrap());
                m
            })
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let resp = client
            .get("https://api.test/things")
            .header("x-req", "r")
            .query(&[("a", "1"), ("b", "x y")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(resp.headers()["x-test"], "1");
        assert!(resp.url().as_str().starts_with("https://api.test/things"));
        assert_eq!(resp.text().await.unwrap(), "hello");
    });
    let r = &seen.borrow()[0];
    assert_eq!(r.method, "GET");
    assert_eq!(r.url, "https://api.test/things?a=1&b=x+y");
    assert_eq!(header(r, "user-agent"), Some("convex-test/1"));
    assert_eq!(header(r, "x-default"), Some("d"));
    assert_eq!(header(r, "x-req"), Some("r"));
    assert!(r.body.is_empty());
}

#[test]
fn json_post_roundtrip_and_auth() {
    let (h, seen) = host(|r| FetchStubResponse::new(201, r.body.clone()));
    let got: Item = h.run(|_| async {
        reqwest::Client::new()
            .post("https://api.test/items")
            .bearer_auth("tok")
            .json(&Item {
                id: 7,
                name: "seven".into(),
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    });
    assert_eq!(
        got,
        Item {
            id: 7,
            name: "seven".into()
        }
    );
    let r = &seen.borrow()[0];
    assert_eq!(r.method, "POST");
    assert_eq!(header(r, "content-type"), Some("application/json"));
    assert_eq!(header(r, "authorization"), Some("Bearer tok"));
}

#[test]
fn form_basic_auth_and_body() {
    let (h, seen) = host(|_| FetchStubResponse::new(204, ""));
    h.run(|_| async {
        let c = reqwest::Client::new();
        c.post("https://api.test/f")
            .basic_auth("user", Some("pw"))
            .form(&[("k", "v 1"), ("z", "&")])
            .send()
            .await
            .unwrap();
        c.put("https://api.test/b")
            .body("raw")
            .send()
            .await
            .unwrap();
    });
    let s = seen.borrow();
    assert_eq!(header(&s[0], "authorization"), Some("Basic dXNlcjpwdw=="));
    assert_eq!(
        header(&s[0], "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(s[0].body, b"k=v+1&z=%26");
    assert_eq!(s[1].method, "PUT");
    assert_eq!(s[1].body, b"raw");
}

#[test]
fn multipart_body() {
    let (h, seen) = host(|_| FetchStubResponse::new(200, ""));
    h.run(|_| async {
        let form = reqwest::multipart::Form::new().text("a", "1").part(
            "f",
            reqwest::multipart::Part::bytes(b"data".to_vec())
                .file_name("x.bin")
                .mime_str("application/octet-stream")
                .unwrap(),
        );
        reqwest::Client::new()
            .post("https://api.test/up")
            .multipart(form)
            .send()
            .await
            .unwrap();
    });
    let r = &seen.borrow()[0];
    let ct = header(r, "content-type").unwrap();
    let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
    let body = String::from_utf8(r.body.clone()).unwrap();
    assert!(body.contains(&format!("--{boundary}\r\n")));
    assert!(body.contains("name=\"a\"\r\n\r\n1\r\n"));
    assert!(body.contains("filename=\"x.bin\""));
    assert!(body.ends_with(&format!("--{boundary}--\r\n")));
}

#[test]
fn error_for_status_and_error_predicates() {
    let (h, _) = host(|_| FetchStubResponse::new(404, "nope"));
    h.run(|_| async {
        let resp = reqwest::get("https://api.test/missing").await.unwrap();
        assert!(resp.error_for_status_ref().is_err());
        let e = resp.error_for_status().unwrap_err();
        assert!(e.is_status());
        assert!(!e.is_timeout());
        assert_eq!(e.status(), Some(reqwest::StatusCode::NOT_FOUND));
        assert_eq!(e.url().unwrap().path(), "/missing");
        assert!(e.to_string().contains("404 Not Found"), "{e}");
    });
    // Bad URL is a builder error.
    let (h, _) = host(|_| FetchStubResponse::new(200, ""));
    h.run(|_| async {
        let e = reqwest::Client::new()
            .get("not a url")
            .send()
            .await
            .unwrap_err();
        assert!(e.is_builder());
    });
}

#[test]
fn fetch_task_failure_is_a_request_error() {
    // No fetch handler: the task fails.
    TestHost::new().run(|_| async {
        let e = reqwest::get("https://api.test/x").await.unwrap_err();
        assert!(e.is_request() && e.is_connect(), "{e:?}");
        assert!(!e.is_timeout());
    });
}

#[test]
fn chunked_body_chunk_and_bytes_stream() {
    let (h, _) = host(|_| FetchStubResponse::chunked(200, ["ab", "cd", "ef"]));
    h.run(|_| async {
        let mut resp = reqwest::get("https://api.test/c").await.unwrap();
        let mut got = vec![];
        while let Some(c) = resp.chunk().await.unwrap() {
            got.push(c);
        }
        assert_eq!(got, ["ab", "cd", "ef"]);

        let resp = reqwest::get("https://api.test/c").await.unwrap();
        let mut s = Box::pin(resp.bytes_stream());
        let mut all = vec![];
        while let Some(c) = s.next().await {
            all.extend_from_slice(&c.unwrap());
        }
        assert_eq!(all, b"abcdef");

        let resp = reqwest::get("https://api.test/c").await.unwrap();
        assert_eq!(&resp.bytes().await.unwrap()[..], b"abcdef");
    });
}

#[test]
fn stream_failure_surfaces_after_chunks() {
    let (h, _) = host(|_| FetchStubResponse::chunked(200, ["ab"]).fail_after("connection reset"));
    h.run(|_| async {
        let mut resp = reqwest::get("https://api.test/c").await.unwrap();
        assert_eq!(&resp.chunk().await.unwrap().unwrap()[..], b"ab");
        let e = resp.chunk().await.unwrap_err();
        assert!(e.is_body());
        assert!(e.to_string().contains("connection reset"), "{e}");
    });
}

#[test]
fn json_decode_error_and_concurrent_requests() {
    let (h, _) =
        host(|r| FetchStubResponse::new(200, if r.url.ends_with("bad") { "{" } else { "[1,2]" }));
    h.run(|_| async {
        let c = reqwest::Client::new();
        let e = c
            .get("https://api.test/bad")
            .send()
            .await
            .unwrap()
            .json::<Vec<u8>>()
            .await
            .unwrap_err();
        assert!(e.is_decode());
        let (a, b) = tokio::join!(
            async {
                c.get("https://api.test/a")
                    .send()
                    .await
                    .unwrap()
                    .json::<Vec<u8>>()
                    .await
                    .unwrap()
            },
            async {
                c.get("https://api.test/b")
                    .send()
                    .await
                    .unwrap()
                    .json::<Vec<u8>>()
                    .await
                    .unwrap()
            },
        );
        assert_eq!((a, b), (vec![1, 2], vec![1, 2]));
    });
}

#[test]
fn https_only_rejects_http() {
    let (h, _) = host(|_| FetchStubResponse::new(200, ""));
    h.run(|_| async {
        let c = reqwest::Client::builder().https_only(true).build().unwrap();
        assert!(c
            .get("http://api.test/")
            .send()
            .await
            .unwrap_err()
            .is_builder());
        assert!(c.get("https://api.test/").send().await.is_ok());
    });
}

#[test]
fn request_build_clone_execute() {
    let (h, seen) = host(|_| FetchStubResponse::new(200, "ok"));
    h.run(|_| async {
        let c = reqwest::Client::new();
        let req = c.post("https://api.test/e").body("x").build().unwrap();
        assert_eq!(req.method(), reqwest::Method::POST);
        let req2 = req.try_clone().unwrap();
        c.execute(req).await.unwrap();
        c.execute(req2).await.unwrap();
    });
    assert_eq!(seen.borrow().len(), 2);
}

#[test]
fn streaming_request_body_is_uploaded() {
    let (h, seen) = host(|_| FetchStubResponse::new(200, "ok"));
    h.run(|_| async {
        let chunks = vec![
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"ab")),
            Ok(bytes::Bytes::from_static(b"cd")),
        ];
        let body = reqwest::Body::wrap_stream(futures_util::stream::iter(chunks));
        let resp = reqwest::Client::new()
            .put("https://api.test/up")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");
    });
    assert_eq!(seen.borrow()[0].body, b"abcd");
}

#[test]
fn async_timeout_fires_on_a_fetch_that_never_completes_and_cancels_it() {
    let h = TestHost::new()
        .with_net_caps()
        .on_fetch(|_| FetchStubResponse::never());
    h.run(|_| async {
        let e = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap()
            .get("https://api.test/hang")
            .send()
            .await
            .unwrap_err();
        assert!(e.is_timeout(), "{e:?}");
    });
    // The hung fetch (task 0) was cancelled on the host.
    assert!(h.cancelled_tasks().contains(&0));
}

/// Inside an action `reqwest::blocking` runs on `block_on_direct`, where no tokio timer is
/// driven: the timeout has to ride on the host `sleep` task.
#[cfg(feature = "blocking")]
#[test]
fn blocking_timeout_fires_inside_an_action() {
    let h = TestHost::new().on_fetch(|_| FetchStubResponse::never());
    h.run(|_| async {
        let e = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_millis(50))
            .build()
            .unwrap()
            .get("https://api.test/hang")
            .send()
            .unwrap_err();
        assert!(e.is_timeout(), "{e:?}");
    });
}

#[test]
fn non_utf8_response_and_request_headers_survive() {
    let (h, seen) = host(|_| FetchStubResponse::new(200, "x").header("x-bin", "caf\u{e9}"));
    h.run(|_| async {
        let resp = reqwest::Client::new()
            .get("https://api.test/h")
            .header(
                "x-in",
                reqwest::header::HeaderValue::from_bytes(b"caf\xe9").unwrap(),
            )
            .send()
            .await
            .unwrap();
        // The ByteString char U+00E9 is the byte 0xE9.
        assert_eq!(resp.headers()["x-bin"].as_bytes(), b"caf\xe9");
    });
    assert_eq!(header(&seen.borrow()[0], "x-in"), Some("caf\u{e9}"));
}

/// With `net/cancel` support the pump blocks on the host instead of polling on a tokio tick;
/// requests, spawned tasks and tokio timers must all still make progress.
#[test]
fn host_blocking_pump_serves_requests_spawned_tasks_and_timers() {
    let (h, _) = host(|r| FetchStubResponse::new(200, r.url.clone()));
    let h = h.with_net_caps();
    h.run(|_| async {
        let spawned = tokio::task::spawn_local(async {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            7
        });
        let c = reqwest::Client::new();
        let (a, b) = tokio::join!(
            async { c.get("https://api.test/a").send().await.unwrap().text().await.unwrap() },
            async { c.get("https://api.test/b").send().await.unwrap().text().await.unwrap() },
        );
        assert_eq!((a.as_str(), b.as_str()), ("https://api.test/a", "https://api.test/b"));
        assert_eq!(spawned.await.unwrap(), 7);
    });
}
