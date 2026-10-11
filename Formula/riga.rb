class Riga < Formula
  desc "Local zsh/iTerm2 autocomplete for macOS"
  homepage "https://github.com/tulma95/riga"
  url "https://github.com/tulma95/riga/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_TARBALL_SHA256"
  license "MIT"
  head "https://github.com/tulma95/riga.git", branch: "main"

  depends_on "rust" => :build
  depends_on :macos

  def install
    system "cargo", "install", *std_cargo_args
    pkgshare.install "shell/riga.zsh"
  end

  def caveats
    <<~EOS
      Add this to ~/.zshrc, after other ZLE customizations:
        source #{opt_pkgshare}/riga.zsh
      Then give iTerm2 Accessibility and Automation permission and run `riga doctor`.
    EOS
  end

  test do
    assert_match "status", shell_output("#{bin}/riga complete 'git sta'")
  end
end
