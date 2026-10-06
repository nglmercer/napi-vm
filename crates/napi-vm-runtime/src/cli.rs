//! Optional command-line runtime. Entry source is host-selected; guest IO and
//! imported dependencies still require explicit grants.
use crate::VmErr;
use crate::runtime::{Runtime, RuntimeBuilder, RuntimeLimits, permissions::Permissions};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;
#[path = "cli/isolation.rs"]
mod isolation;

const HELP: &str = "napi-vm run [permissions] app.js\nnapi-vm eval [permissions] '1 + 2'\nnapi-vm repl [permissions]\nnapi-vm info\nnapi-vm install [permissions] [--locked] npm:package\n\nPermissions: --allow-read=DIR --allow-write=DIR --allow-net=HOST[:PORT]\n             --allow-env=NAME --allow-ffi --allow-run\nLimits: --isolate=process --max-memory=128M --max-cpu=5s --max-time=5s --max-jobs=N --max-stack=N --max-fuel=N\n        --max-io=N --max-timers=N --max-file-bytes=N\nOptional: --compat=node (requires runtime-node)\nFilesystem globals (runtime-fs): napiVm.readTextFile / writeTextFile\nEnvironment: napiVm.env(NAME)\n";

fn error(message: impl Into<String>) -> VmErr {
    VmErr::Msg(message.into())
}
fn duration(value: &str) -> Result<Duration, VmErr> {
    let (number, scale) = if let Some(n) = value.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = value.strip_suffix('s') {
        (n, 1.0)
    } else {
        return Err(error("time must end in ms or s"));
    };
    let seconds: f64 = number
        .parse::<f64>()
        .map_err(|_| error("invalid time limit"))?
        * scale;
    Duration::try_from_secs_f64(seconds).map_err(|_| error("invalid time limit"))
}
fn number(value: &str) -> Result<usize, VmErr> {
    value.parse().map_err(|_| error("invalid numeric limit"))
}

fn network_grant(input: &str) -> Result<(String, Option<u16>), VmErr> {
    let url =
        url::Url::parse(&format!("http://{input}")).map_err(|_| error("invalid network grant"))?;
    if url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error(
            "network grants must contain only a host and optional port",
        ));
    }
    // Url::port() removes an explicit default HTTP port. Preserve the user's
    // port restriction instead of accidentally turning :80 into an all-port grant.
    let port_text = if input.starts_with('[') {
        let end = input.find(']').ok_or_else(|| error("invalid IPv6 grant"))?;
        let rest = &input[end + 1..];
        if rest.is_empty() {
            None
        } else {
            Some(
                rest.strip_prefix(':')
                    .ok_or_else(|| error("invalid network grant"))?,
            )
        }
    } else {
        input.rsplit_once(':').map(|(_, port)| port)
    };
    let port = port_text
        .map(|text| {
            text.parse::<u16>()
                .map_err(|_| error("invalid network port"))
        })
        .transpose()?;
    Ok((
        url.host_str()
            .ok_or_else(|| error("missing network host"))?
            .into(),
        port,
    ))
}

fn make_runtime(arguments: &[String]) -> Result<(Runtime, Vec<String>), VmErr> {
    let mut permissions = Permissions::new();
    let mut limits = RuntimeLimits::default();
    let mut operands = Vec::new();
    let mut node = false;
    let mut positional = false;
    for argument in arguments {
        if positional {
            operands.push(argument.clone());
            continue;
        }
        if argument == "--" {
            positional = true;
            continue;
        }
        if let Some(path) = argument.strip_prefix("--allow-read=") {
            permissions = permissions
                .allow_read(path)
                .map_err(|e| error(e.to_string()))?;
        } else if let Some(path) = argument.strip_prefix("--allow-write=") {
            permissions = permissions
                .allow_write(path)
                .map_err(|e| error(e.to_string()))?;
        } else if let Some(host) = argument.strip_prefix("--allow-net=") {
            let (host, port) = network_grant(host)?;
            permissions = permissions.allow_net(host, port);
        } else if let Some(name) = argument.strip_prefix("--allow-env=") {
            if name.is_empty() {
                return Err(error("environment grant requires a name"));
            }
            permissions = permissions.allow_env(name);
        } else if argument == "--allow-ffi" {
            permissions = permissions.allow_ffi();
        } else if argument == "--allow-run" {
            permissions = permissions.allow_process();
        } else if argument == "--compat=node" {
            node = true;
        } else if let Some(value) = argument.strip_prefix("--max-time=") {
            limits.timeout = Some(duration(value)?);
        } else if let Some(value) = argument.strip_prefix("--max-jobs=") {
            limits.jobs = number(value)?;
        } else if let Some(value) = argument.strip_prefix("--max-stack=") {
            limits.stack_depth = number(value)?;
        } else if let Some(value) = argument.strip_prefix("--max-fuel=") {
            limits.fuel = value.parse().map_err(|_| error("invalid fuel limit"))?;
        } else if let Some(value) = argument.strip_prefix("--max-timers=") {
            limits.timers = number(value)?;
        } else if let Some(value) = argument.strip_prefix("--max-io=") {
            limits.io_resources = number(value)?;
        } else if let Some(value) = argument.strip_prefix("--max-file-bytes=") {
            limits.file_bytes = number(value)?;
        } else if argument.starts_with("--") {
            return Err(error(format!("unsupported option: {argument}")));
        } else {
            operands.push(argument.clone());
        }
    }
    let builder = RuntimeBuilder::new()
        .console()
        .timers()
        .environment()
        .permissions(permissions.clone())
        .limits(limits.clone());
    #[cfg(feature = "runtime-web")]
    let builder = builder.web_apis();
    #[cfg(feature = "runtime-fs")]
    let builder = builder.filesystem(permissions.clone());
    #[cfg(feature = "runtime-net")]
    let builder = builder.network(permissions.clone());
    #[cfg(feature = "runtime-npm")]
    let builder = builder.npm_packages();
    let builder = if node {
        #[cfg(feature = "runtime-node")]
        {
            builder.node_compat()
        }
        #[cfg(not(feature = "runtime-node"))]
        return Err(error("Node compatibility requires runtime-node"));
    } else {
        builder
    };
    let mut runtime = builder.build_runtime()?;
    #[cfg(feature = "runtime-npm")]
    let fallback = crate::runtime::npm::NpmLoader::new(
        permissions.clone(),
        std::env::current_dir().map_err(|e| error(e.to_string()))?,
        limits.file_bytes,
    )
    .map_err(|e| error(e.to_string()))?;
    #[cfg(feature = "runtime-node")]
    let fallback = if node {
        fallback.node_builtins()
    } else {
        fallback
    };
    #[cfg(feature = "runtime-npm")]
    let fallback: Rc<dyn crate::ModuleLoader> = Rc::new(fallback);
    #[cfg(all(feature = "runtime-fs", not(feature = "runtime-npm")))]
    let fallback: Rc<dyn crate::ModuleLoader> = Rc::new(
        crate::runtime::loaders::FileLoader::new(
            permissions.clone(),
            std::env::current_dir().map_err(|e| error(e.to_string()))?,
            limits.file_bytes,
        )
        .map_err(|e| error(e.to_string()))?,
    );
    #[cfg(not(any(feature = "runtime-fs", feature = "runtime-npm")))]
    let fallback: Rc<dyn crate::ModuleLoader> = Rc::new(crate::VirtualLoader::new());
    let loader = crate::CompositeLoader::new(fallback).with_scheme(
        "data",
        Rc::new(crate::DataUrlLoader::new(limits.file_bytes)),
    );
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    let loader = {
        let http: Rc<dyn crate::ModuleLoader> = Rc::new(crate::runtime::loaders::HttpLoader::new(
            permissions,
            limits.file_bytes,
            limits.timeout.unwrap_or(Duration::from_secs(30)),
        ));
        loader
            .with_scheme("http", http.clone())
            .with_scheme("https", http)
    };
    let loader: Rc<dyn crate::ModuleLoader> = Rc::new(loader);
    #[cfg(feature = "runtime-typescript")]
    let loader: Rc<dyn crate::ModuleLoader> =
        Rc::new(crate::runtime::typescript::TypeScriptLoader::new(loader));
    runtime.interpreter_mut().set_module_loader(loader);
    Ok((runtime, operands))
}
fn evaluate(runtime: &mut Runtime, source: &str, print: bool) -> Result<(), VmErr> {
    let value = runtime.eval(source)?;
    runtime.run_event_loop()?;
    if print {
        println!("{}", runtime.interpreter().vs(&value)?);
    }
    Ok(())
}
fn run_file(runtime: &mut Runtime, filename: &str) -> Result<(), VmErr> {
    use std::io::Read;
    let path = Path::new(filename)
        .canonicalize()
        .map_err(|e| error(e.to_string()))?;
    #[cfg(not(feature = "runtime-typescript"))]
    if matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("ts" | "tsx")
    ) {
        return Err(error("TypeScript requires runtime-typescript"));
    }
    let mut source = String::new();
    // Entry text is host-selected, with a fixed hard cap independent of guest grants.
    std::fs::File::open(&path)
        .map_err(|e| error(e.to_string()))?
        .take(16 * 1024 * 1024 + 1)
        .read_to_string(&mut source)
        .map_err(|e| error(e.to_string()))?;
    if source.len() > 16 * 1024 * 1024 {
        return Err(error("entry source exceeds 16 MiB"));
    }
    if source.starts_with("#!") {
        source = source.replacen("#!", "//", 1);
    }
    #[cfg(feature = "runtime-typescript")]
    if path
        .extension()
        .is_some_and(|e| matches!(e.to_str(), Some("ts" | "tsx" | "mts")))
    {
        source = crate::runtime::typescript::transform(&path, &source)?.javascript;
    }
    let id = url::Url::from_file_path(&path)
        .map_err(|_| error("invalid entry path"))?
        .to_string();
    runtime.interpreter_mut().define_module(&id, source);
    runtime
        .interpreter_mut()
        .define_module_file_url(&id, id.clone());
    runtime.run_module(&id)?;
    runtime.run_event_loop()
}
fn execute() -> Result<(), VmErr> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if let Some(code) = isolation::supervise(&arguments)? {
        if code != 0 {
            return Err(error(format!("Isolated runtime exited with status {code}")));
        }
        return Ok(());
    }
    let Some(command) = arguments.first() else {
        print!("{HELP}");
        return Ok(());
    };
    if matches!(command.as_str(), "--help" | "-h" | "help") {
        print!("{HELP}");
        return Ok(());
    }
    if command == "info" {
        println!(
            "napi-vm {}\nTest262 initial development baseline: 31.43%\nWeb/Node/npm compatibility: not measured\nRuntime: opt-in\nPermissions: denied by default\nFeatures: web={} net={} fs={} npm={} node={} typescript={}",
            env!("CARGO_PKG_VERSION"),
            cfg!(feature = "runtime-web"),
            cfg!(feature = "runtime-net"),
            cfg!(feature = "runtime-fs"),
            cfg!(feature = "runtime-npm"),
            cfg!(feature = "runtime-node"),
            cfg!(feature = "runtime-typescript")
        );
        return Ok(());
    }
    if command == "install" {
        #[cfg(all(
            feature = "runtime-npm",
            feature = "runtime-net",
            not(target_arch = "wasm32")
        ))]
        {
            let mut locked = false;
            let mut registry = "https://registry.npmjs.org/".to_string();
            let mut root = std::env::current_dir().map_err(|e| error(e.to_string()))?;
            let mut options = Vec::new();
            for option in &arguments[1..] {
                if option == "--locked" {
                    locked = true;
                } else if let Some(value) = option.strip_prefix("--registry=") {
                    registry = value.into();
                } else if let Some(value) = option.strip_prefix("--root=") {
                    root = value.into();
                } else {
                    options.push(option.clone());
                }
            }
            let (runtime, operands) = make_runtime(&options)?;
            if operands.len() != 1 {
                return Err(error("install requires one package specifier"));
            }
            let mut installer = crate::runtime::npm::NpmInstaller::new(
                runtime.permissions().clone(),
                root,
                &registry,
                runtime.limits().file_bytes,
                locked,
            )?;
            let lock = installer.install(&operands[0])?;
            println!(
                "Installed {} package artifacts with verified integrity",
                lock.packages.len()
            );
            return Ok(());
        }
        #[cfg(not(all(
            feature = "runtime-npm",
            feature = "runtime-net",
            not(target_arch = "wasm32")
        )))]
        return Err(error(
            "Package installation requires runtime-npm and runtime-net",
        ));
    }
    if !matches!(command.as_str(), "run" | "eval" | "repl") {
        return Err(error(format!("unknown command: {command}")));
    }
    let (mut runtime, operands) = make_runtime(&arguments[1..])?;
    match command.as_str() {
        "run" if operands.len() == 1 => run_file(&mut runtime, &operands[0]),
        "eval" if operands.len() == 1 => evaluate(&mut runtime, &operands[0], true),
        "repl" if operands.is_empty() => {
            use std::io::IsTerminal;
            let terminal = io::stdin().is_terminal();
            let stdin = io::stdin();
            let mut lines = stdin.lock().lines();
            loop {
                if terminal {
                    print!("> ");
                    io::stdout().flush().map_err(|e| error(e.to_string()))?;
                }
                let Some(line) = lines.next() else { break };
                let line = line.map_err(|e| error(e.to_string()))?;
                if line.trim() == ".exit" {
                    break;
                }
                if line.trim().is_empty() {
                    continue;
                }
                if let Err(error) = evaluate(&mut runtime, &line, true) {
                    eprintln!("{error}");
                }
            }
            Ok(())
        }
        _ => Err(error("wrong number of operands; use --help")),
    }
}
pub fn main() {
    if let Err(error) = execute() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::network_grant;
    #[test]
    fn network_grants_preserve_explicit_default_ports() {
        assert_eq!(
            network_grant("example.com:80").unwrap(),
            ("example.com".into(), Some(80))
        );
        assert_eq!(
            network_grant("example.com").unwrap(),
            ("example.com".into(), None)
        );
        assert_eq!(network_grant("[::1]:80").unwrap().1, Some(80));
        assert_eq!(network_grant("[::1]").unwrap().1, None);
        assert!(network_grant("example.com:").is_err());
        assert!(network_grant("example.com/path").is_err());
    }
}
