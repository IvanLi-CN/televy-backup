# frozen_string_literal: true

cask "televybackup" do
  version "0.9.11"
  sha256 "fdf009e32b6e30e51661b030bd82cd82dbea08883f7ec0dca2e1cb8c13a57bff"

  url "https://github.com/IvanLi-CN/televy-backup/releases/download/v#{version}/TelevyBackup-#{version}.dmg"
  name "TelevyBackup"
  desc "Encrypted backup client for macOS"
  homepage "https://github.com/IvanLi-CN/televy-backup"

  depends_on macos: :sequoia

  app "TelevyBackup.app"

  caveats do
    <<~EOS
      TelevyBackup releases are ad-hoc signed and are not notarized by Apple. Homebrew
      leaves macOS quarantine intact. After verifying the download, open the app from
      Finder and approve it through macOS Gatekeeper if prompted.
    EOS
  end

  livecheck do
    url :url
    strategy :github_latest
  end
end
