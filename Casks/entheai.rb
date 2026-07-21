cask "entheai" do
  version "0.1.0"
  sha256 :no_check # ad-hoc-signed self-built zip; pin the sha on a real release

  url "https://github.com/entropy-om/entheai/releases/download/v#{version}/entheai-app-macos-arm64.zip"
  name "entheai"
  desc "Native minimalist Ghostty window running the entheai coding agent"
  homepage "https://entheai.com/"

  depends_on cask: "ghostty"
  depends_on macos: :ventura
  depends_on arch: :arm64

  app "entheai.app"

  caveats <<~EOS
    entheai.app is ad-hoc signed. On first launch: right-click the app -> Open,
    then confirm. (Notarized signing is planned.)
  EOS
end
