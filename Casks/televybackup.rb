# frozen_string_literal: true

cask "televybackup" do
  version "0.9.8"
  sha256 "56180c32798b74be199c3bcbbef0f025107fd93859651f81aef80d1770a7ced8"

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
