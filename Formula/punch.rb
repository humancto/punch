class Punch < Formula
  desc "The Agent Combat System — deploy autonomous AI agent squads from a single binary"
  homepage "https://github.com/humancto/punch"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/humancto/punch/releases/download/v#{version}/punch-aarch64-apple-darwin.tar.gz"
      # sha256 "UPDATE_AFTER_RELEASE"
    else
      url "https://github.com/humancto/punch/releases/download/v#{version}/punch-x86_64-apple-darwin.tar.gz"
      # sha256 "UPDATE_AFTER_RELEASE"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/humancto/punch/releases/download/v#{version}/punch-aarch64-unknown-linux-gnu.tar.gz"
      # sha256 "UPDATE_AFTER_RELEASE"
    else
      url "https://github.com/humancto/punch/releases/download/v#{version}/punch-x86_64-unknown-linux-gnu.tar.gz"
      # sha256 "UPDATE_AFTER_RELEASE"
    end
  end

  def install
    bin.install "punch"
  end

  test do
    assert_match "punch", shell_output("#{bin}/punch --version")
  end
end
