# Homebrew formula for entheai. This repo doubles as its own tap:
#   brew tap entropy-om/entheai https://github.com/entropy-om/entheai
#   brew install entheai
#
# macOS / Apple Silicon only.
class Entheai < Formula
  desc "macOS-native hybrid coding agent CLI with autonomous multi-agent fan-out and cognitive memory"
  homepage "https://vaked.dev"
  version "42.1.12"
  license "MIT"

  depends_on :macos
  depends_on arch: :arm64

  url "https://github.com/entropy-om/entheai/archive/refs/tags/v42.1.12.tar.gz"
  sha256 "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"

  head "https://github.com/entropy-om/entheai.git", branch: "main"

  def install
    bin.install "bin/entheai" if File.exist?("bin/entheai")
    bin.install "bin/entheai-companion" if File.exist?("bin/entheai-companion")
    bin.install "bin/entheai-worker" if File.exist?("bin/entheai-worker")
  end

  test do
    assert_match "entheai", shell_output("#{bin}/entheai --help")
  end
end
