cask "televybackup" do
  version "0.1.0"
  sha256 :no_check

  url "https://github.com/IvanLi-CN/televy-backup/releases/download/v#{version}/TelevyBackup.dmg"
  name "TelevyBackup"
  desc "macOS desktop app for Telegram-backed encrypted backups"
  homepage "https://github.com/IvanLi-CN/televy-backup"

  app "TelevyBackup.app"
end

