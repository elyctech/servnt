use std::{
    collections::HashMap,
    env,
    error::Error,
    fmt, fs, io,
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
    #[serde(default = "HashMap::new")]
    extensions: HashMap<String, String>,
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

fn default_extension_content_types() -> HashMap<String, String> {
    [
        ("html", "text/html"),
        ("png", "image/png"),
        ("ico", "image/vnd.microsoft.icon"),
        ("webmanifest", "application/manifest+json"),
    ]
    .map(|(extension, content_type)| (extension.to_string(), content_type.to_string()))
    .into()
}

enum FileError {
    IoError(io::Error),
    UnknownExtension,
}

impl From<io::Error> for FileError {
    fn from(error: io::Error) -> Self {
        FileError::IoError(error)
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::IoError(io_error) => io_error.fmt(f),
            FileError::UnknownExtension => f.write_str("unknown extension"),
        }
    }
}

struct ServntState {
    extension_content_types: HashMap<String, String>,
    full_base_path: PathBuf,
    mapped_paths: HashMap<String, PathBuf>,
}

impl ServntState {
    fn new(cwd: &PathBuf, servnt_file: ServntFile) -> Result<ServntState, io::Error> {
        let mut extension_content_types = default_extension_content_types();

        for (extension, content_type) in servnt_file.extensions {
            extension_content_types.insert(extension, content_type);
        }

        let full_base_path = cwd.join(&servnt_file.paths.base).canonicalize()?;

        let mut mapped_paths = HashMap::with_capacity(servnt_file.paths.mapped.len());

        for (matched, mapped) in servnt_file.paths.mapped {
            mapped_paths.insert(matched, cwd.join(&mapped).canonicalize()?);
        }

        Ok(ServntState {
            extension_content_types,
            full_base_path,
            mapped_paths,
        })
    }

    fn get_content_type<P>(&self, path: P) -> Result<String, FileError>
    where
        P: AsRef<Path>,
    {
        self.extension_content_types
            .get(
                path.as_ref()
                    .extension()
                    .map_or(Err(FileError::UnknownExtension), |extension| {
                        extension.to_str().ok_or(FileError::UnknownExtension)
                    })?,
            )
            .ok_or(FileError::UnknownExtension)
            .cloned()
    }

    fn resolve_path<P>(&self, path: P) -> Result<PathBuf, io::Error>
    where
        P: AsRef<Path>,
    {
        let match_path = Path::new("/").join(&path);
        let mut final_path = None;

        for (matched, mapped) in &self.mapped_paths {
            if let Ok(stripped_path) = match_path.strip_prefix(matched) {
                if stripped_path == Path::new("") {
                    final_path = Some(mapped.clone());
                } else {
                    final_path = Some(mapped.join(stripped_path));
                }

                break;
            }
        }

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
        .route("/", get(get_root_index))
        .route("/*desired", get(get_path))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 19518));

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn get_file(state: &ServntState, path: &str) -> Result<impl IntoResponse, impl IntoResponse> {
    state
        .resolve_path(path)
        .map_err(FileError::IoError)
        .and_then(|file_path| {
            Ok(state
                .get_content_type(&file_path)
                .map(|content_type| (content_type, file_path))?)
        })
        .and_then(|(content_type, file_path)| {
            Ok(([("Content-Type", content_type)], fs::read(file_path)?))
        })
        .or_else(|error| {
            eprintln!("{error}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        })
}

async fn get_path(
    extract::Path(desired_path): extract::Path<String>,
    State(state): State<Arc<ServntState>>,
) -> impl IntoResponse {
    get_file(&state, &desired_path).await
}

async fn get_root_index(State(state): State<Arc<ServntState>>) -> impl IntoResponse {
    get_file(&state, "index.html").await
}
