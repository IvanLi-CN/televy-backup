class Televybackupd < Formula
  desc "TelevyBackup scheduled backup daemon"
  homepage "https://github.com/IvanLi-CN/televy-backup"
  url "https://github.com/IvanLi-CN/televy-backup/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "cd18341b59128d01d4550046d4904d2cb55479dcdcc229bd687b90a4e5d6c0e4"
  license "Apache-2.0"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/daemon")
  end

  def post_install
    (etc/"televybackup").mkpath
    (var/"lib/televybackup").mkpath
    (var/"log").mkpath
  end

  service do
    run [opt_bin/"televybackupd"]
    keep_alive true
    working_dir var
    environment_variables(
      TELEVYBACKUP_CONFIG_DIR: etc/"televybackup",
      TELEVYBACKUP_DATA_DIR: var/"lib/televybackup"
    )
    log_path var/"log/televybackupd.log"
    error_log_path var/"log/televybackupd.log"
  end

  def caveats
    <<~EOS
      Config: #{etc}/televybackup/config.toml
      Data:   #{var}/lib/televybackup/index/index.sqlite
      Logs:   #{var}/log/televybackupd.log

      The daemon requires macOS Keychain secrets:
      - Telegram bot token key: "telegram.bot_token" (default)
      - Master key: "televybackup.master_key"
    EOS
  end
end
