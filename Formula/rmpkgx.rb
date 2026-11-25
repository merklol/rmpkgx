# Generated from the exact SOURCE release asset, not a prebuilt binary archive.
class Rmpkgx < Formula
  desc "Conservative offline macOS application and leftover inspector"
  homepage "https://github.com/merklol/rmpkgx"
  url "https://github.com/merklol/rmpkgx/releases/download/v0.1.0/rmpkgx-v0.1.0-source.tar.gz"
  version "0.1.0"
  sha256 "27bca3ca3991f11ca20fbde79eefacc8d9dfb995222e8870490b2f46f9eb2632"
  license "MIT"

  depends_on "rust" => :build
  depends_on :macos
  depends_on arch: :arm64

  deny_network_access!

  def fetch
    system "cargo", "fetch", *std_cargo_fetch_args
  end

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_equal "rmpkgx #{version}", shell_output("#{bin}/rmpkgx --version").strip
    assert_match "orphans", shell_output("#{bin}/rmpkgx --help")
    # No destructive operations or scans of real Library data in brew tests.
    assert_match "interactive terminal required", shell_output("#{bin}/rmpkgx 2>&1", 1)
  end
end
