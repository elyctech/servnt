use std::{
    collections::HashMap,
    env,
    error::Error,
    fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    extract::{self, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct ServntFile {
    app: AppInfo,
    paths: AppPaths,
}

#[derive(Deserialize)]
struct AppInfo {
    name: String,
    version: String,
}

#[derive(Deserialize)]
struct AppPaths {
    #[serde(default = "AppPaths::default_base")]
    base: String,
    mapped: HashMap<String, String>,
}

impl AppPaths {
    fn default_base() -> String {
        "src".to_string()
    }
}

struct ServntState {
    full_base_path: PathBuf,
    mapped_paths: HashMap<String, PathBuf>,
}

impl ServntState {
    fn new(cwd: &PathBuf, servnt_file: ServntFile) -> Result<ServntState, io::Error> {
        let full_base_path = cwd.join(&servnt_file.paths.base).canonicalize()?;
        let mut mapped_paths = HashMap::with_capacity(servnt_file.paths.mapped.len());

        for (matched, mapped) in servnt_file.paths.mapped {
            mapped_paths.insert(matched, cwd.join(&mapped).canonicalize()?);
        }

        Ok(ServntState {
            full_base_path,
            mapped_paths,
        })
    }

    fn resolve_path<P>(&self, path: P) -> Result<PathBuf, io::Error>
    where
        P: AsRef<Path>,
    {
        let match_path = Path::new("/").join(&path);
        let mut final_path = None;

        println!("Looking for '{}'", match_path.display());

        for (matched, mapped) in &self.mapped_paths {
            println!("checking for match with '{}'", matched);
            if let Ok(stripped_path) = match_path.strip_prefix(matched) {
                if stripped_path == Path::new("") {
                    final_path = Some(mapped.clone());
                } else {
                    final_path = Some(mapped.join(stripped_path));
                }

                break;
            }
        }

        println!(
            "{} => '{}'",
            path.as_ref().display(),
            final_path
                .clone()
                .unwrap_or(self.full_base_path.join(&path).join("!"))
                .display()
        );

        final_path
            .unwrap_or_else(|| self.full_base_path.join(&path))
            .canonicalize()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cwd = env::current_dir()?;
    let servnt_file = toml::from_str::<ServntFile>(&fs::read_to_string(&cwd.join("servnt.toml"))?)?;

    println!(
        "Serving app '{} (v{})'",
        servnt_file.app.name, servnt_file.app.version
    );

    let state = Arc::new(ServntState::new(&cwd, servnt_file)?);

    let app = Router::new()
        .route("/", get(get_index))
        .route("/:desired", get(get_path))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 19518));

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn get_file(state: &ServntState, path: &str) -> impl IntoResponse {
    match state.resolve_path(path) {
        Ok(index_path) => (
            StatusCode::OK,
            format!("You wanted '{}'", index_path.display()),
        ),
        Err(error) => {
            eprintln!("{error}");
            (StatusCode::INTERNAL_SERVER_ERROR, "".to_string())
        }
    }
}

async fn get_index(State(state): State<Arc<ServntState>>) -> impl IntoResponse {
    get_file(&state, "index.html").await
}

async fn get_path(
    extract::Path(desired_path): extract::Path<String>,
    State(state): State<Arc<ServntState>>,
) -> impl IntoResponse {
    get_file(&state, &desired_path).await
}
