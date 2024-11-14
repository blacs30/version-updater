use core::fmt;
use std::env;
use std::fs::File;
use std::io::{read_to_string, Write};
use std::path::{self, PathBuf};
use std::process::exit;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, USER_AGENT};

const GITHUB_BASE_API_URL: &str = "https://api.github.com/repos";
const GITHUB_LATEST_RELEASE_ENDPOINT: &str = "releases/latest";
const GITLAB_BASE_URL: &str = "https://gitlab.com";
const GITLAB_LATEST_RELEASE_ENDPOINT: &str = "releases/permalink/latest";

#[derive(Error, Debug)]
pub enum Error {
    #[error("http client error {0}")]
    HttpClientError(String),
    #[error("http server error {0}")]
    HttpServerError(String),
    #[error("http error {0}")]
    HttpReqwestError(String),
    #[error("Failed to serialze release result {0}")]
    SerializeError(String),
    #[error("Unkown repo type {0}")]
    UnknownRepoType(String),
}

#[derive(Serialize, Deserialize)]
struct ReleaseInfo {
    version: String,
    url: String,
}

#[derive(Serialize, Deserialize)]
struct GitHubReleaseInfo {
    name: String,
    html_url: String,
}

#[derive(Serialize, Deserialize)]
struct GitLabReleaseInfo {
    tag_name: String,
    #[serde(rename = "_links")]
    links: Links,
}

#[derive(Serialize, Deserialize)]
struct Links {
    #[serde(rename = "self")]
    url: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum RepoType {
    GitLab,
    GitHub,
}
impl fmt::Display for RepoType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoType::GitHub => write!(f, "GitHub"),
            RepoType::GitLab => write!(f, "GitLab"),
        }
    }
}

// Example file content could look this way:
// - name: Wallabag
//   repo: wallabag/wallabag
//   repo_type: GitHub
//   project_id: null
//   version: 2.6.10
//   auth: true
// - name: Comentario
//   repo: comentario/comentario
//   repo_type: GitLab
//   project_id: '42486427'
//   version: v3.11.0
//   auth: null

/// TODO
// # // TODO: docker check
// # // check_image: true
// # // image_path: bla/blubb
// # // image_auth: true
// # // image_version_regex: "^123$"

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Services {
    name: String,
    repo: String,
    repo_type: RepoType,
    project_id: Option<String>,
    version: String,
    auth: Option<bool>,
    image_check: Option<ImageCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImageCheck {
    enable_check: Option<bool>,
    auth: Option<bool>,
    reg_path: Option<String>,
    version_rebex: Option<String>,
}

struct VersionFileUpdater {
    file_name: PathBuf,
}

impl VersionFileUpdater {
    fn new(file: String) -> Self {
        let file_path =
            path::PathBuf::from_str(file.as_str()).expect("Expected a path to a version file.");
        VersionFileUpdater {
            file_name: file_path,
        }
    }
    fn load_file(&self) -> Vec<Services> {
        let mut file = File::open(&self.file_name).expect("Unable to open file");
        let contents = read_to_string(&mut file).expect("Unable to read file");

        let services: Vec<Services> =
            serde_yaml::from_str(&contents).expect("Could not load content into YAML");
        services
    }

    fn update_versions(&self, http_client: &Client) -> Vec<Services> {
        let mut updated_service = false;
        let mut repos = self.load_file();
        for service in repos.iter_mut() {
            let project_ref: String = match service.repo_type {
                RepoType::GitHub => service.repo.clone(),
                RepoType::GitLab => service.project_id.clone().unwrap(),
            };
            let result =
                match get_release_info(http_client, service.repo_type, project_ref, service.auth) {
                    Ok(res) => res,
                    Err(e) => {
                        println!("error occured: {}", e);
                        exit(1)
                    }
                };
            let old_version = service.version.clone();
            if old_version != result.version.clone() {
                service.version = result.version.clone();
                updated_service = true;
                print_version(service.clone(), result, old_version);
            }
        }
        if !updated_service {
            println!("No new versions for services available.");
        }
        repos
    }
    fn update_file(&self, repos: Vec<Services>) {
        let mut file = File::create(&self.file_name).expect("Unable to open file");
        let s = serde_yaml::to_string(&repos).expect("Not valid yaml");
        match file.write_all(&s.into_bytes()) {
            Ok(..) => {}
            Err(e) => println!("{}", e),
        };
    }
}

fn print_version(service: Services, result: ReleaseInfo, old_version: String) {
    println!(
        "===============\n\
        {} repo {} for service {}\n\
        Old version was: {}.\n\
        New version is {}.\n\
        Release information can be found here {}",
        service.repo_type, service.repo, service.name, old_version, result.version, result.url
    );
}

fn get_release_info(
    client: &Client,
    repo_type: RepoType,
    project: String,
    auth: Option<bool>,
) -> Result<ReleaseInfo, Error> {
    let (url, headers) = match repo_type {
        RepoType::GitHub => {
            let url = format!(
                "{}/{}/{}",
                GITHUB_BASE_API_URL, project, GITHUB_LATEST_RELEASE_ENDPOINT
            );
            let headers = construct_headers();
            (url, headers)
        }
        RepoType::GitLab => {
            let project_api_url = format!("{}/{}/{}", GITLAB_BASE_URL, "api/v4/projects", project);
            let url = format!("{}/{}", project_api_url, GITLAB_LATEST_RELEASE_ENDPOINT);
            let mut headers = construct_headers();
            if auth.is_some() == true {
                let gitlab_header = String::from("PRIVATE-TOKEN");
                let gitlab_header = gitlab_header.as_str();
                let gitlab_token = env::var("GITLAB_TOKEN")
                .expect(format!("Expected an environment variable with name GITLAB_TOKEN to access private Gitlab project with id {}.", project).as_str());
                headers.insert(
                    HeaderName::from_bytes(gitlab_header.as_bytes()).unwrap(),
                    format!("{}", gitlab_token)
                        .try_into()
                        .expect("invalid characters"),
                );
            };
            (url, headers)
        }
    };

    let res = match client.get(url).headers(headers).send() {
        Ok(res) => res,
        Err(e) => return Err(Error::HttpReqwestError(e.to_string())),
    };

    let status = res.status().as_u16();
    match status {
        status if (200..300).contains(&status) => {
            let res_text = res.text().unwrap();
            // serialize
            match repo_type {
                RepoType::GitLab => {
                    let result: GitLabReleaseInfo = match serde_json::from_str(res_text.as_str()) {
                        Ok(value) => value,
                        Err(e) => return Err(Error::SerializeError(e.to_string())),
                    };
                    let rl = ReleaseInfo {
                        version: result.tag_name,
                        url: result.links.url,
                    };
                    Ok(rl)
                }
                RepoType::GitHub => {
                    let result: GitHubReleaseInfo = match serde_json::from_str(res_text.as_str()) {
                        Ok(value) => value,
                        Err(e) => return Err(Error::SerializeError(e.to_string())),
                    };
                    let rl = ReleaseInfo {
                        version: result.name,
                        url: result.html_url,
                    };
                    Ok(rl)
                }
            }
        }
        status if (400..500).contains(&status) => Err(Error::HttpClientError(format!(
            "{:?}",
            res.error_for_status_ref()
        ))),
        status if (500..600).contains(&status) => Err(Error::HttpServerError(format!(
            "{:?}",
            res.error_for_status_ref()
        ))),
        _ => Err(Error::HttpReqwestError(format!("{}", res.status()))),
    }
}

fn construct_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Accept",
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(USER_AGENT, HeaderValue::from_static("reqwest"));
    headers
}

fn main() {
    let http_client = Client::new();
    let version_file_name = env::var("VERSION_FILE")
        .expect("Expected an environment variable with name VERSION_FILE to update version.");
    let version_file = VersionFileUpdater::new(version_file_name);
    let repos = version_file.update_versions(&http_client);
    version_file.update_file(repos);
}
