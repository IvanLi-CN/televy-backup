# frozen_string_literal: true

cask "televybackup" do
  version "0.9.10"
  sha256 "e9e9980835ea546ece1157159e801365a37de67cc5a6a5bb2f70c3ee6fabe970"

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
