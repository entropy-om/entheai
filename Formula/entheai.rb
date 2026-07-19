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
  version "0.1.0"
  license "MIT"

  depends_on :macos
  depends_on arch: :arm64

  url "https://github.com/peterlodri-sec/entheai/releases/download/v0.1.0/entheai-macos-arm64.tar.gz"
  sha256 "10f010e7599cc31bc73d24f50e0841c68f94e0404a0f901c8165c0e34ce56ab7"

  def install
    bin.install "entheai"
    bin.install "entheai-companion"
  end

  test do
    assert_match "entheai 0.1.0", shell_output("#{bin}/entheai --version")
  end
end
