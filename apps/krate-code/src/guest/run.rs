//! The editor's runner on `krate:process` (IC-977).
//!
//! Each program runs in the project folder the person picked
//! (`picked/<token>`), which the host resolves; the editor never sees a
//! path. Nothing here waits except [`Runner::run_to_end`], which a save
//! uses for rustfmt and gives up after its deadline: output is read as it
//! arrives each time the event loop turns.

use crate::tools::{Finished, JobEvent, RunError, Runner};
use alloc::string::String;
use alloc::vec::Vec;
use krate::process::{self, Process, ProcessError};

/// The most read from one stream in one turn, so one chatty program cannot
/// hold up the frame.
const READ_MAX: u32 = 1 << 20;

struct Proc {
    p: Process,
    out_done: bool,
    err_done: bool,
    exited: bool,
}

#[derive(Default)]
pub struct ProcessRunner {
    folder: Option<String>,
    procs: Vec<Proc>,
}

fn why(e: ProcessError) -> RunError {
    let w = String::from(process::words(&e));
    match e {
        ProcessError::NotFound(_) => RunError::Missing(w),
        ProcessError::NotDeclared(_) | ProcessError::RefusedByPerson(_) => RunError::Refused(w),
        _ => RunError::Other(w),
    }
}

impl ProcessRunner {
    fn cwd(&self, sub: Option<&str>) -> Result<String, RunError> {
        let Some(f) = &self.folder else {
            return Err(RunError::Other(String::from(crate::tools::NEEDS_FOLDER)));
        };
        let mut s = f.clone();
        if let Some(sub) = sub.filter(|s| !s.is_empty()) {
            s.push('/');
            s.push_str(sub);
        }
        Ok(s)
    }

    fn find(&self, job: u64) -> Option<&Proc> {
        self.procs.iter().find(|p| p.p.handle() == job)
    }
}

impl Runner for ProcessRunner {
    fn set_folder(&mut self, folder: Option<String>) {
        self.folder = folder;
    }

    fn folder(&self) -> Option<&str> {
        self.folder.as_deref()
    }

    fn start(
        &mut self,
        program: &str,
        args: &[String],
        sub: Option<&str>,
    ) -> Result<u64, RunError> {
        let cwd = self.cwd(sub)?;
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let p = Process::start(program, &a, &cwd).map_err(why)?;
        let id = p.handle();
        self.procs.push(Proc {
            p,
            out_done: false,
            err_done: false,
            exited: false,
        });
        Ok(id)
    }

    fn terminal(&mut self, cols: u16, rows: u16) -> Result<u64, RunError> {
        let cwd = self.cwd(None)?;
        let p = Process::terminal(&cwd, cols, rows).map_err(why)?;
        let id = p.handle();
        self.procs.push(Proc {
            p,
            out_done: false,
            err_done: false,
            exited: false,
        });
        Ok(id)
    }

    fn poll(&mut self) -> Vec<JobEvent> {
        let mut ev = Vec::new();
        for pr in self.procs.iter_mut().filter(|p| !p.exited) {
            let job = pr.p.handle();
            if !pr.out_done {
                match pr.p.read_stdout(READ_MAX) {
                    Ok(o) => {
                        if !o.bytes.is_empty() {
                            ev.push(JobEvent::Output {
                                job,
                                bytes: o.bytes,
                                stderr: false,
                            });
                        }
                        pr.out_done = o.ended;
                    }
                    Err(_) => pr.out_done = true,
                }
            }
            if !pr.err_done {
                match pr.p.read_stderr(READ_MAX) {
                    Ok(o) => {
                        if !o.bytes.is_empty() {
                            ev.push(JobEvent::Output {
                                job,
                                bytes: o.bytes,
                                stderr: true,
                            });
                        }
                        pr.err_done = o.ended;
                    }
                    Err(_) => pr.err_done = true,
                }
            }
            if pr.out_done && pr.err_done {
                match pr.p.wait(0) {
                    Ok(Some(s)) => {
                        pr.exited = true;
                        ev.push(JobEvent::Exited {
                            job,
                            code: s.code,
                            signal: s.signal,
                        });
                    }
                    Ok(None) => {}
                    Err(_) => {
                        pr.exited = true;
                        ev.push(JobEvent::Exited {
                            job,
                            code: None,
                            signal: None,
                        });
                    }
                }
            }
        }
        ev
    }

    fn write(&mut self, job: u64, bytes: &[u8]) -> Result<(), String> {
        match self.find(job) {
            Some(p) => {
                p.p.write(bytes)
                    .map_err(|e| String::from(process::words(&e)))
            }
            None => Err(String::from("that program is not running")),
        }
    }

    fn close_stdin(&mut self, job: u64) {
        if let Some(p) = self.find(job) {
            let _ = p.p.close_stdin();
        }
    }

    fn resize(&mut self, job: u64, cols: u16, rows: u16) {
        if let Some(p) = self.find(job) {
            let _ = p.p.resize(cols, rows);
        }
    }

    fn kill(&mut self, job: u64) {
        if let Some(p) = self.find(job) {
            let _ = p.p.kill();
        }
    }

    fn release(&mut self, job: u64) {
        // Dropping the process releases it with the host.
        self.procs.retain(|p| p.p.handle() != job);
    }

    fn run_to_end(
        &mut self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout_ms: u32,
    ) -> Result<Finished, RunError> {
        let cwd = self.cwd(None)?;
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let p = Process::start(program, &a, &cwd).map_err(why)?;
        if !input.is_empty() {
            let _ = p.write(input);
        }
        let _ = p.close_stdin();
        let deadline = krate::time::now_millis() + timeout_ms as u64;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut od, mut ed) = (false, false);
        loop {
            if !od {
                match p.read_stdout(READ_MAX) {
                    Ok(o) => {
                        out.extend_from_slice(&o.bytes);
                        od = o.ended;
                    }
                    Err(_) => od = true,
                }
            }
            if !ed {
                match p.read_stderr(READ_MAX) {
                    Ok(o) => {
                        err.extend_from_slice(&o.bytes);
                        ed = o.ended;
                    }
                    Err(_) => ed = true,
                }
            }
            if od && ed {
                break;
            }
            if krate::time::now_millis() > deadline {
                let _ = p.kill();
                return Ok(Finished {
                    code: None,
                    stdout: out,
                    stderr: err,
                });
            }
            let _ = p.wait(5);
        }
        let left = deadline.saturating_sub(krate::time::now_millis()).max(50) as u32;
        let code = match p.wait(left) {
            Ok(Some(s)) => s.code,
            _ => {
                let _ = p.kill();
                None
            }
        };
        Ok(Finished {
            code,
            stdout: out,
            stderr: err,
        })
    }
}
