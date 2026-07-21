// Buildx bake definition, mirroring the per-app docker-bake.hcl layout in
// AtvikSecurity/containers. CI (.github/workflows/docker.yml) drives the
// `image` target and injects tags/labels via docker/metadata-action; the
// `docker-metadata-action` target below is the empty hook it overrides.
//
// Local build: `docker buildx bake` (defaults to image-local -> domarinn:local).

target "docker-metadata-action" {}

variable "APP" {
  default = "domarinn"
}

variable "SOURCE" {
  default = "https://github.com/AtvikSecurity/domarinn"
}

group "default" {
  targets = ["image-local"]
}

target "image" {
  inherits = ["docker-metadata-action"]
  labels = {
    "org.opencontainers.image.source" = "${SOURCE}"
  }
}

target "image-local" {
  inherits = ["image"]
  output = ["type=docker"]
  tags = ["${APP}:local"]
}

target "image-all" {
  inherits = ["image"]
  platforms = [
    "linux/amd64",
    // "linux/arm64"  // Disabled - no arm64 runners available
  ]
}
