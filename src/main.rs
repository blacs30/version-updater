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
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};

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

struct VersionFile {
    file_name: PathBuf,
}

impl VersionFile {
    fn new(file: String) -> Self {
        let file_path =
            path::PathBuf::from_str(file.as_str()).expect("Excepted a path to a version file.");
        VersionFile {
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
        let mut repos = self.load_file();
        for service in repos.iter_mut() {
            let project_ref: String = match service.repo_type {
                RepoType::GitHub => service.repo.clone(),
                RepoType::GitLab => service.project_id.clone().unwrap(),
            };
            let result = match get_release_info(http_client, service.repo_type, project_ref) {
                Ok(res) => res,
                Err(e) => {
                    println!("error occured: {}", e);
                    exit(1)
                }
            };
            service.version = result.version.clone();
            print_version(service.clone(), result);
        }
        repos
    }
    fn write_file(&self, repos: Vec<Services>) {
        let mut file = File::create(&self.file_name).expect("Unable to open file");
        let s = serde_yaml::to_string(&repos).expect("Not valid yaml");
        match file.write_all(&s.into_bytes()) {
            Ok(..) => {}
            Err(e) => println!("{}", e),
        };
    }
}

fn print_version(service: Services, result: ReleaseInfo) {
    println!(
        "===============\n\
        {} repo {} for service {}\n\
        Version is {}.\n\
        Release information can be found here {}",
        service.repo_type, service.repo, service.name, result.version, result.url
    );
}

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

const GITHUB_BASE_API_URL: &str = "https://api.github.com/repos";
const GITHUB_LATEST_RELEASE_ENDPOINT: &str = "releases/latest";
const GITLAB_BASE_URL: &str = "https://gitlab.com";
const GITLAB_LATEST_RELEASE_ENDPOINT: &str = "releases/permalink/latest";

fn get_release_info(
    client: &Client,
    repo_type: RepoType,
    project: String,
) -> Result<ReleaseInfo, Error> {
    let url: String = match repo_type {
        RepoType::GitHub => format!(
            "{}/{}/{}",
            GITHUB_BASE_API_URL, project, GITHUB_LATEST_RELEASE_ENDPOINT
        ),
        RepoType::GitLab => {
            let project_api_url = format!("{}/{}/{}", GITLAB_BASE_URL, "api/v4/projects", project);
            format!("{}/{}", project_api_url, GITLAB_LATEST_RELEASE_ENDPOINT)
        }
    };

    let res = match client.get(url).headers(construct_headers()).send() {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Services {
    name: String,
    repo: String,
    repo_type: RepoType,
    project_id: Option<String>,
    version: String,
}

fn main() {
    let http_client = Client::new();
    let version_file_name = env::var("VERSION_FILE")
        .expect("Expect an environment variable with name VERSION_FILE to update version.");
    let version_file = VersionFile::new(version_file_name);
    let repos = version_file.update_versions(&http_client);
    version_file.write_file(repos);
}
