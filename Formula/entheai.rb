# Homebrew formula for entheai. This repo doubles as its own tap:
#   brew tap peterlodri-sec/entheai https://github.com/peterlodri-sec/entheai
#   brew install entheai
#
# macOS / Apple Silicon only. It ships a PREBUILT binary because GitHub-hosted
# macOS runners are unavailable for this project — the release tarball
# (entheai + entheai-companion) is built locally, PGO-optimized when the
# toolchain permits (else the optimized release profile), and attached to the
# matching GitHub release. On a new release: bump `version`, rebuild + upload the
# tarball, and update `sha256` to the new tarball's hash.
class Entheai < Formula
  desc "Hybrid, visual, self-improving terminal coding-agent harness"
  homepage "https://entheai.com"
  version "0.2.1"
  license "MIT"

  depends_on :macos
  depends_on arch: :arm64

  url "https://github.com/peterlodri-sec/entheai/releases/download/v0.2.1/entheai-macos-arm64.tar.gz"
  sha256 "d909827f490761585e3ed53dd8878bba80eee72f439bd06efd1b685eeee76902"

  def install
    bin.install "entheai"
    bin.install "entheai-companion"
  end

  test do
    assert_match "entheai 0.2.1", shell_output("#{bin}/entheai --version")
  end
end
