// Buildx bake definition for local and ad-hoc builds.
//
// CI does NOT use this file: .github/workflows/docker.yml delegates to Docker's
// reusable build workflow, which drives the Dockerfile directly and derives its
// own tags, labels and annotations. This exists so `docker buildx bake` gives a
// developer the same image locally without reproducing that pipeline by hand.
//
// Local build: `docker buildx bake` (defaults to image-local -> domarinn:local).
// Multi-arch:  `docker buildx bake image-all` (needs a buildx builder with
//              QEMU/binfmt or a remote arm64 node).

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
    "linux/arm64",
  ]
}
