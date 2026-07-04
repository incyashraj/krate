use std::{hint::black_box, time::Duration};

use criterion::{criterion_group, criterion_main, Criterion};
use krate_policy::SessionPolicy;
use krate_runtime::{
    uapi::UapiGuard,
    uapi_dispatch::{
        AdapterError, DateStyle, FileHandle, FileStat, FsAdapter, Header, HostAdapter, HttpRequest,
        HttpResponse, IoAdapter, LocaleAdapter, LocaleId, NetAdapter, OpenMode, TimeAdapter,
        UapiDispatcher,
    },
};

fn phase2_uapi_dispatch_benches(c: &mut Criterion) {
    let adapter = NoopAdapter;
    let default_guard = UapiGuard::new(SessionPolicy::default());
    let fs_policy = SessionPolicy::from_cli_grants(&[
        "fs.read:./notes/**".to_string(),
        "fs.write:./notes/**".to_string(),
    ])
    .expect("fs benchmark policy");
    let fs_guard = UapiGuard::new(fs_policy);
    let net_policy = SessionPolicy::from_cli_grants(&["net.connect:127.0.0.1:80".to_string()])
        .expect("net benchmark policy");
    let net_guard = UapiGuard::new(net_policy);

    let default_dispatcher = UapiDispatcher::new(&default_guard, &adapter);
    let fs_dispatcher = UapiDispatcher::new(&fs_guard, &adapter);
    let net_dispatcher = UapiDispatcher::new(&net_guard, &adapter);
    let read_handle = FileHandle::opened_file(7, "./notes/today.txt", OpenMode::Read);
    let write_handle = FileHandle::opened_file(8, "./notes/today.txt", OpenMode::ReadWrite);

    let mut group = c.benchmark_group("phase2_uapi_dispatch");
    group
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(5));

    group.bench_function("default_stdout_grant", |b| {
        b.iter(|| {
            black_box(default_dispatcher.stdout().expect("stdout grant"));
        });
    });

    group.bench_function("fs_open_read_granted", |b| {
        b.iter(|| {
            black_box(
                fs_dispatcher
                    .fs_open(black_box("./notes/today.txt"), OpenMode::Read)
                    .expect("fs read grant"),
            );
        });
    });

    group.bench_function("fs_handle_read_granted", |b| {
        b.iter(|| {
            black_box(
                fs_dispatcher
                    .fs_read(black_box(&read_handle), black_box(4096))
                    .expect("fs read handle grant"),
            );
        });
    });

    group.bench_function("fs_handle_write_granted", |b| {
        b.iter(|| {
            black_box(
                fs_dispatcher
                    .fs_write(black_box(&write_handle), black_box(b"hello"))
                    .expect("fs write handle grant"),
            );
        });
    });

    group.bench_function("fs_missing_read_denied", |b| {
        b.iter(|| {
            black_box(
                default_dispatcher
                    .fs_open(black_box("./notes/today.txt"), OpenMode::Read)
                    .expect_err("fs read should be denied"),
            );
        });
    });

    group.bench_function("net_fetch_granted", |b| {
        let req = HttpRequest {
            method: krate_runtime::uapi_dispatch::HttpMethod::Get,
            url: "http://127.0.0.1/health".to_string(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout_millis: Some(1000),
        };
        b.iter(|| {
            black_box(
                net_dispatcher
                    .net_fetch(black_box(req.clone()))
                    .expect("net grant"),
            );
        });
    });

    group.finish();
}

struct NoopAdapter;

impl HostAdapter for NoopAdapter {
    fn io(&self) -> &dyn IoAdapter {
        self
    }

    fn fs(&self) -> &dyn FsAdapter {
        self
    }

    fn net(&self) -> &dyn NetAdapter {
        self
    }

    fn time(&self) -> &dyn TimeAdapter {
        self
    }

    fn locale(&self) -> &dyn LocaleAdapter {
        self
    }
}

impl IoAdapter for NoopAdapter {
    fn stdin(&self) -> Result<FileHandle, AdapterError> {
        Ok(FileHandle::resource(1))
    }

    fn stdout(&self) -> Result<FileHandle, AdapterError> {
        Ok(FileHandle::resource(2))
    }

    fn stderr(&self) -> Result<FileHandle, AdapterError> {
        Ok(FileHandle::resource(3))
    }

    fn args_raw(&self) -> Result<String, AdapterError> {
        Ok("notes.txt".to_string())
    }

    fn read_stream(&self, _handle: &FileHandle, _n: u32) -> Result<Vec<u8>, AdapterError> {
        Ok(Vec::new())
    }

    fn read_stream_to_string(&self, _handle: &FileHandle) -> Result<String, AdapterError> {
        Ok(String::new())
    }

    fn write_stream(&self, _handle: &FileHandle, bytes: &[u8]) -> Result<u32, AdapterError> {
        Ok(bytes.len() as u32)
    }

    fn write_all_stream(&self, _handle: &FileHandle, _bytes: &[u8]) -> Result<(), AdapterError> {
        Ok(())
    }

    fn flush_stream(&self, _handle: &FileHandle) -> Result<(), AdapterError> {
        Ok(())
    }

    fn close_stream(&self, _handle: &FileHandle) -> Result<(), AdapterError> {
        Ok(())
    }

    fn log(&self, _level: &str, _message: &str) -> Result<(), AdapterError> {
        Ok(())
    }
}

impl FsAdapter for NoopAdapter {
    fn open(&self, _path: &str, _mode: OpenMode) -> Result<FileHandle, AdapterError> {
        Ok(FileHandle::resource(4))
    }

    fn read(&self, _handle: &FileHandle, n: u32) -> Result<Vec<u8>, AdapterError> {
        Ok(vec![0; n as usize])
    }

    fn write(&self, _handle: &FileHandle, bytes: &[u8]) -> Result<u32, AdapterError> {
        Ok(bytes.len() as u32)
    }

    fn seek_set(&self, _handle: &FileHandle, pos: u64) -> Result<u64, AdapterError> {
        Ok(pos)
    }

    fn seek_end(&self, _handle: &FileHandle) -> Result<u64, AdapterError> {
        Ok(0)
    }

    fn stat_handle(&self, _handle: &FileHandle) -> Result<FileStat, AdapterError> {
        Ok(FileStat {
            size: 0,
            modified_millis: 0,
            is_dir: false,
        })
    }

    fn stat(&self, _path: &str) -> Result<FileStat, AdapterError> {
        Ok(FileStat {
            size: 0,
            modified_millis: 0,
            is_dir: false,
        })
    }

    fn list(&self, _path: &str) -> Result<Vec<String>, AdapterError> {
        Ok(Vec::new())
    }

    fn remove_file(&self, _path: &str) -> Result<(), AdapterError> {
        Ok(())
    }

    fn remove_dir(&self, _path: &str) -> Result<(), AdapterError> {
        Ok(())
    }

    fn mkdir(&self, _path: &str) -> Result<(), AdapterError> {
        Ok(())
    }

    fn rename(&self, _from: &str, _to: &str) -> Result<(), AdapterError> {
        Ok(())
    }

    fn close_file(&self, _handle: &FileHandle) -> Result<(), AdapterError> {
        Ok(())
    }
}

impl NetAdapter for NoopAdapter {
    fn fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AdapterError> {
        Ok(HttpResponse {
            status: 200,
            headers: Vec::<Header>::new(),
            body: Vec::new(),
        })
    }
}

impl TimeAdapter for NoopAdapter {
    fn now_millis(&self) -> Result<u64, AdapterError> {
        Ok(0)
    }

    fn monotonic_nanos(&self) -> Result<u64, AdapterError> {
        Ok(0)
    }

    fn sleep_millis(&self, _millis: u32) -> Result<(), AdapterError> {
        Ok(())
    }
}

impl LocaleAdapter for NoopAdapter {
    fn current(&self) -> Result<LocaleId, AdapterError> {
        Ok(LocaleId {
            bcp47: "en-US".to_string(),
        })
    }

    fn timezone(&self) -> Result<String, AdapterError> {
        Ok("UTC".to_string())
    }

    fn format_date(
        &self,
        _millis: u64,
        _tz: &str,
        _style: DateStyle,
        _loc: &LocaleId,
    ) -> Result<String, AdapterError> {
        Ok("date".to_string())
    }

    fn format_number(
        &self,
        _value: f64,
        _style: krate_runtime::uapi_dispatch::NumberStyle,
        _loc: &LocaleId,
    ) -> Result<String, AdapterError> {
        Ok("number".to_string())
    }
}

criterion_group!(benches, phase2_uapi_dispatch_benches);
criterion_main!(benches);
