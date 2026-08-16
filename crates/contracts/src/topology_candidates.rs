use super::*;

pub(super) fn static_candidates(files: &BTreeMap<String, String>) -> Vec<ServiceCandidate> {
    files
        .keys()
        .filter(|path| path.ends_with("index.html"))
        .filter_map(|path| {
            let directory = parent_directory(path);
            let has_manifest = files.contains_key(&join_directory(&directory, "package.json"));
            (!has_manifest).then(|| ServiceCandidate {
                selector: format!("static:{path}"),
                name: directory.rsplit('/').next().unwrap_or("static").to_string(),
                role: ServiceRole::Frontend,
                root_directory: directory.clone(),
                provider: "staticfile".to_string(),
                package_manager: None,
                build_command: None,
                start_command: None,
                output_directory: Some(directory),
                container_port: 80,
                health_probe: HealthProbe {
                    kind: HealthProbeKind::Http,
                    path: Some("/health".to_string()),
                },
                public_env: Vec::new(),
                evidence: vec!["index.html".to_string()],
            })
        })
        .collect()
}

pub(super) fn python_candidates(files: &BTreeMap<String, String>) -> Vec<ServiceCandidate> {
    files
        .iter()
        .filter(|(path, _)| path.ends_with("pyproject.toml") || path.ends_with("requirements.txt"))
        .filter_map(|(path, contents)| {
            let lower = contents.to_ascii_lowercase();
            let framework = [
                "fastapi",
                "starlette",
                "flask",
                "django",
                "gunicorn",
                "uvicorn",
            ]
            .into_iter()
            .find(|framework| lower.contains(framework))?;
            let directory = parent_directory(path);
            let (start, health) = if lower.contains("django") {
                (
                    "gunicorn --bind 0.0.0.0:$PORT config.wsgi:application".to_string(),
                    HealthProbeKind::Http,
                )
            } else if lower.contains("flask") {
                (
                    "gunicorn --bind 0.0.0.0:$PORT app:app".to_string(),
                    HealthProbeKind::Http,
                )
            } else {
                (
                    "uvicorn main:app --host 0.0.0.0 --port $PORT".to_string(),
                    HealthProbeKind::Http,
                )
            };
            Some(ServiceCandidate {
                selector: format!("python:{path}"),
                name: directory
                    .rsplit('/')
                    .next()
                    .unwrap_or("python-app")
                    .to_string(),
                role: ServiceRole::Backend,
                root_directory: directory.clone(),
                provider: "python".to_string(),
                package_manager: None,
                build_command: None,
                start_command: Some(in_directory(&directory, &start)),
                output_directory: None,
                container_port: 3000,
                health_probe: HealthProbe {
                    kind: health,
                    path: Some("/".to_string()),
                },
                public_env: Vec::new(),
                evidence: vec![format!("dependency {framework}")],
            })
        })
        .collect()
}

pub(super) fn go_candidates(files: &BTreeMap<String, String>) -> Vec<ServiceCandidate> {
    files
        .keys()
        .filter(|path| path.ends_with("go.mod"))
        .filter_map(|path| {
            let directory = parent_directory(path);
            let has_main = files.iter().any(|(candidate, contents)| {
                candidate.starts_with(&format!("{}/", directory.trim_end_matches('.')))
                    && candidate.ends_with(".go")
                    && contents.contains("package main")
            }) || (directory == "."
                && files.iter().any(|(candidate, contents)| {
                    candidate.ends_with(".go") && contents.contains("package main")
                }));
            has_main.then(|| ServiceCandidate {
                selector: format!("golang:{path}"),
                name: directory.rsplit('/').next().unwrap_or("go-app").to_string(),
                role: ServiceRole::Backend,
                root_directory: directory.clone(),
                provider: "golang".to_string(),
                package_manager: None,
                build_command: Some(in_directory(&directory, "go build -o /app/hostlet-go .")),
                start_command: Some("/app/hostlet-go".to_string()),
                output_directory: None,
                container_port: 3000,
                health_probe: HealthProbe {
                    kind: HealthProbeKind::Http,
                    path: Some("/".to_string()),
                },
                public_env: Vec::new(),
                evidence: vec!["go.mod and package main".to_string()],
            })
        })
        .collect()
}

pub(super) fn rust_candidates(files: &BTreeMap<String, String>) -> Vec<ServiceCandidate> {
    files
        .iter()
        .filter(|(path, _)| path.ends_with("Cargo.toml"))
        .filter_map(|(path, contents)| {
            let directory = parent_directory(path);
            let main_path = join_directory(&directory, "src/main.rs");
            let has_main = files.contains_key(&main_path);
            if !has_main || contents.contains("[workspace]") && !contents.contains("[package]") {
                return None;
            }
            let name = toml_string(contents, "name").unwrap_or_else(|| {
                directory
                    .rsplit('/')
                    .next()
                    .unwrap_or("rust-app")
                    .to_string()
            });
            Some(ServiceCandidate {
                selector: format!("rust:{path}:{name}"),
                name: name.clone(),
                role: ServiceRole::Backend,
                root_directory: directory.clone(),
                provider: "rust".to_string(),
                package_manager: None,
                build_command: Some(in_directory(&directory, "cargo build --release")),
                start_command: Some(format!(
                    "/app/{}/target/release/{name}",
                    directory.trim_start_matches("./")
                )),
                output_directory: None,
                container_port: 3000,
                health_probe: HealthProbe {
                    kind: HealthProbeKind::Http,
                    path: Some("/".to_string()),
                },
                public_env: Vec::new(),
                evidence: vec!["Cargo binary target".to_string()],
            })
        })
        .collect()
}
