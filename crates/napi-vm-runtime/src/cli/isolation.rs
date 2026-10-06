//! Supervisor limits are installed in the child before any guest code runs.
use crate::VmErr;
#[cfg(unix)]
use std::time::Duration;
fn err(message: impl Into<String>) -> VmErr {
    VmErr::Msg(message.into())
}
fn memory(input: &str) -> Result<u64, VmErr> {
    let (number, scale) = if let Some(n) = input.strip_suffix('M') {
        (n, 1024 * 1024)
    } else if let Some(n) = input.strip_suffix('G') {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = input.strip_suffix('K') {
        (n, 1024)
    } else {
        (input, 1)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale))
        .filter(|n| *n > 0)
        .ok_or_else(|| err("Invalid memory limit"))
}
pub(super) fn supervise(arguments: &[String]) -> Result<Option<i32>, VmErr> {
    let mut isolate = false;
    let mut heap = None;
    let mut cpu = None;
    let mut wall = None;
    let mut child_arguments = Vec::new();
    let mut positional = false;
    for argument in arguments {
        if positional {
            child_arguments.push(argument.clone());
            continue;
        }
        if argument == "--" {
            positional = true;
            child_arguments.push(argument.clone());
            continue;
        }
        if argument == "--isolate=process" {
            isolate = true;
        } else if argument.starts_with("--isolate=") {
            return Err(err("Unsupported isolation mode"));
        } else if let Some(input) = argument.strip_prefix("--max-memory=") {
            heap = Some(memory(input)?);
            isolate = true;
        } else if let Some(input) = argument.strip_prefix("--max-cpu=") {
            cpu = Some(super::duration(input)?);
            isolate = true;
        } else {
            if let Some(input) = argument.strip_prefix("--max-time=") {
                wall = Some(super::duration(input)?);
            }
            child_arguments.push(argument.clone());
        }
    }
    if !isolate {
        return Ok(None);
    }
    if !matches!(arguments.first().map(String::as_str), Some("run" | "eval")) {
        return Err(err("Process isolation supports run and eval"));
    }
    #[cfg(not(unix))]
    {
        let _ = (heap, cpu, wall, child_arguments);
        Err(err("Process resource limits require Unix on this build"))
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut command =
            std::process::Command::new(std::env::current_exe().map_err(|e| err(e.to_string()))?);
        command.args(child_arguments);
        // SAFETY: only async-signal-safe setrlimit syscalls and simple arithmetic
        // run between fork and exec; no locks or heap allocation in this closure.
        unsafe {
            command.pre_exec(move || {
                if let Some(memory) = heap {
                    let limit = libc::rlimit {
                        rlim_cur: memory as libc::rlim_t,
                        rlim_max: memory as libc::rlim_t,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if let Some(cpu) = cpu {
                    let seconds = cpu
                        .as_secs()
                        .saturating_add(u64::from(cpu.subsec_nanos() > 0))
                        .max(1);
                    let limit = libc::rlimit {
                        rlim_cur: seconds as libc::rlim_t,
                        rlim_max: seconds.saturating_add(1) as libc::rlim_t,
                    };
                    if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let deadline = match wall {
            Some(duration) => Some(
                std::time::Instant::now()
                    .checked_add(duration)
                    .ok_or_else(|| err("Invalid supervisor timeout"))?,
            ),
            None => None,
        };
        let mut child = command.spawn().map_err(|e| err(e.to_string()))?;
        loop {
            if let Some(status) = child.try_wait().map_err(|e| err(e.to_string()))? {
                use std::os::unix::process::ExitStatusExt;
                return Ok(Some(
                    status
                        .code()
                        .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)),
                ));
            }
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err("ResourceLimit: supervisor execution timeout exceeded"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
