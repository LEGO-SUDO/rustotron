# Homebrew formula template for the rustotron tap.
#
# Copy this into the LEGO-SUDO/homebrew-rustotron repo at
# Formula/rustotron.rb. cargo-dist will REGENERATE this file on every
# release once you run `cargo dist init` and tag v0.1.0 — the version
# below is just a placeholder so users can see the eventual shape.
#
# Manual install (until the tap repo exists):
#   brew tap LEGO-SUDO/rustotron https://github.com/LEGO-SUDO/homebrew-rustotron
#   brew install rustotron

class Rustotron < Formula
  desc "Terminal-native network inspector for React Native apps (Reactotron-compatible)"
  homepage "https://github.com/LEGO-SUDO/rustotron"
  version "0.1.0"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/LEGO-SUDO/rustotron/releases/download/v#{version}/rustotron-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
    on_intel do
      url "https://github.com/LEGO-SUDO/rustotron/releases/download/v#{version}/rustotron-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/LEGO-SUDO/rustotron/releases/download/v#{version}/rustotron-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
    on_intel do
      url "https://github.com/LEGO-SUDO/rustotron/releases/download/v#{version}/rustotron-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_RELEASE_SHA256"
    end
  end

  def install
    bin.install "rustotron"
  end

  test do
    # Smoke test: --version exits 0 with the expected version string.
    assert_match version.to_s, shell_output("#{bin}/rustotron --version")
  end
end
