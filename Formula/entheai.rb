# Homebrew formula for entheai. This repo doubles as its own tap:
#   brew tap entropy-om/entheai https://github.com/entropy-om/entheai
#   brew install entheai
#
# macOS / Apple Silicon only. It ships PREBUILT binaries because GitHub-hosted
# macOS runners are unavailable for this project — the release binaries
# (entheai + entheai-worker) are built locally, PGO-optimized when the
# toolchain permits (else the optimized release profile), and attached to the
# matching GitHub release. On a new release: bump `version`, upload the fresh
# binaries, and update each `sha256` to the new binary's hash.
class Entheai < Formula
  desc "MacOS-native hybrid coding agent CLI with fan-out and cognitive memory"
  homepage "https://vaked.dev"
  version "42.1.11"
  license "MIT"

  depends_on :macos
  depends_on arch: :arm64

  url "https://github.com/entropy-om/entheai/releases/download/v42.1.11/entheai"
  sha256 "89e2deba0f8a611cc822c12ed14222487c2ade4212ee1090a4c106091135d2d0"

  resource "worker" do
    url "https://github.com/entropy-om/entheai/releases/download/v42.1.11/entheai-worker"
    sha256 "ba8c7177e6118ac3362be65efdcb123a93d0c8948d0f69dd9200bce57e105d05"
  end

  def install
    bin.install "entheai"
    resource("worker").stage { bin.install "entheai-worker" }
  end

  test do
    assert_match "entheai 42.1", shell_output("#{bin}/entheai --version")
  end
end
