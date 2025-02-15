# Version Updater

A service that automatically checks and validates the latest versions for existance of Docker images based on Git repository releases.

## Purpose

Version Updater helps you keep track of Docker image versions that correspond to Git repository releases. It:

- Fetches the latest release versions from GitHub or GitLab repositories
- Validates if corresponding Docker images exist in container registries
- Outputs the results in JSON or YAML format

This is particularly useful for:

- CI/CD pipelines that need to validate image versions
- Automated version checking of microservices
- Maintaining consistency between Git releases and Docker images

## Installation

```bash
cargo install version-updater
```

## Usage

1. Create a configuration file (e.g., `config.yaml`):

```yaml
global:
  git:
    github:
      authenticate: true # Set to true if you need authenticated GitHub API access

services:
  my-service:
    git:
      type: github
      repo: organization/repository
      version_filter: "v(.*)" # Optional: regex to extract version from tag
      private: false
    image:
      name: ghcr.io/organization/image-name
      tag: "${RELEASE_VERSION}" # ${RELEASE_VERSION} will be replaced with the extracted version

  gitlab-service:
    git:
      type: gitlab
      project_id: 12345
      private: true
    image:
      name: registry.gitlab.com/organization/image-name
      tag: "v${RELEASE_VERSION}"
```

2. Run the tool:

```bash
version-updater -c config.yaml -o output.json
```

## Configuration

### Environment Variables

- `GITHUB_TOKEN`: Required for private GitHub repositories or when `github.authenticate` is true
- `GITLAB_TOKEN`: Required for private GitLab repositories
- `RUST_LOG`: Controls log level (error, warn, info, debug, trace)

### Command Line Options

- `-c, --config`: Path to config file (default: config.yaml)
- `-f, --format`: Output format (json or yaml, default: json)
- `-o, --output`: Output file path (required)

## Tested With Providers

### Git Providers

- GitHub
- GitLab

### Container Registries

- Docker Hub
- GitHub Container Registry (ghcr.io)
- GitLab Container Registry
- Quay.io

## How It Works

1. **Configuration Loading**

   - Loads and validates the configuration file
   - Checks for required environment variables

2. **Version Detection**

   - Queries Git provider APIs to get the latest release version
   - Applies version filter regex if specified
   - Handles rate limiting and authentication

3. **Image Validation**

   - Constructs image tags using the detected version
   - Validates image existence in the container registry
   - Handles registry authentication using Docker credentials

4. **Output Generation**
   - Processes all services concurrently
   - Generates a structured output with results
   - Supports JSON and YAML formats

## Example Output

```json
{
  "my-service": {
    "image": "ghcr.io/organization/image-name",
    "tag": "1.2.3"
  },
  "gitlab-service": {
    "image": "registry.gitlab.com/organization/image-name",
    "tag": "v2.0.1"
  }
}
```

### Using Output with Other Tools

The generated output is designed to be easily consumed by other infrastructure tooling:

#### Terraform Example

```hcl
locals {
  versions = jsondecode(file("versions.json"))
}


resource "nomad_job" "my_service" {
  jobspec = templatefile("${path.module}/job.hcl", {
    image = "${local.versions.my-service.image}"
    tag   = "${local.versions.my-service.tag}"
  })
}
```

## Error Handling

- Missing images are marked with `<NOT_FOUND>`
- Rate limited requests are marked with `<RATE_LIMITED>`
- Other errors include an error message in the output

## Development

Requirements:

- Rust 1.56 or later
- Docker (for accessing registry credentials)

Building:

```bash
cargo build --release
```

Testing:

```bash
cargo test
```

## License

[MIT License](LICENSE)
