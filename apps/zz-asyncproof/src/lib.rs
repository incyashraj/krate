//! K-101 proof: does the app stay alive while a slow request runs?
//!
//! Starts a request against a deliberately slow server, then counts how many
//! times it gets to do work before the answer arrives. On the blocking path
//! that count is zero by construction -- the guest is inside `fetch` and
//! cannot run at all. On the async path it should be large.
#![no_std]
extern crate alloc;

mod bindings;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{io, net, time};

bindings::export!(App with_types_in bindings);
struct App;

impl bindings::Guest for App {
    fn run() -> i32 {
        let args: Vec<String> = io::args::all();
        let url = args
            .iter()
            .find(|a| a.starts_with("http"))
            .cloned()
            .unwrap_or_else(|| String::from("http://127.0.0.1:8799/"));

        let req = net::Request {
            method: net::HttpMethod::Get,
            url: url.clone(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout_millis: Some(20_000),
        };

        let started = time::monotonic_nanos();
        let handle = match net::begin(req) {
            Ok(h) => h,
            Err(e) => {
                let _ = io::stdio::println(&format!("begin_failed:{e:?}"));
                return 1;
            }
        };
        let began_after = (time::monotonic_nanos() - started) / 1_000_000;
        let _ = io::stdio::println(&format!("begin_returned_after_ms:{began_after}"));

        // The measurement: how much work happens while the request is in
        // flight. Each turn is what a real app would spend drawing a frame.
        let mut turns: u64 = 0;
        loop {
            match net::poll(handle) {
                net::FetchStatus::Pending => {
                    turns += 1;
                    time::sleep_millis(10);
                }
                net::FetchStatus::Ready(res) => {
                    let _ = io::stdio::println(&format!("status:{}", res.status));
                    let _ = io::stdio::println(&format!("body_bytes:{}", res.body.len()));
                    break;
                }
                net::FetchStatus::Failed(e) => {
                    let _ = io::stdio::println(&format!("failed:{e:?}"));
                    break;
                }
                net::FetchStatus::UnknownHandle => {
                    let _ = io::stdio::println("unknown_handle");
                    break;
                }
            }
        }
        let total = (time::monotonic_nanos() - started) / 1_000_000;
        let _ = io::stdio::println(&format!("turns_while_waiting:{turns}"));
        let _ = io::stdio::println(&format!("total_ms:{total}"));
        0
    }
}
