#!/bin/sh
# Publish a release and point the Homebrew formula at it: scripts/release.sh 0.2.0
set -eu
version=${1:?usage: scripts/release.sh VERSION (for example 0.2.0)}
tag="v$version"
repo=tulma95/riga
cd "$(dirname "$0")/.."

[ "$(git branch --show-current)" = main ] || { echo "Not on main" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "Working tree is not clean" >&2; exit 1; }
git fetch -q origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "main is not in sync with origin/main" >&2; exit 1; }
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || { echo "Tag $tag already exists" >&2; exit 1; }

if ! grep -q "^version = \"$version\"$" Cargo.toml; then
  sed -i '' "s/^version = \".*\"$/version = \"$version\"/" Cargo.toml
  cargo check -q  # updates Cargo.lock
  git commit -q -am "Release $tag"
fi
cargo test -q
git tag "$tag"
git push -q origin main "$tag"

url="https://github.com/$repo/archive/refs/tags/$tag.tar.gz"
sha=$(curl -fsSL "$url" | shasum -a 256 | cut -d' ' -f1)
sed -i '' -e "s|^  url \".*\"$|  url \"$url\"|" -e "s|^  sha256 \".*\"$|  sha256 \"$sha\"|" Formula/riga.rb
git commit -q -am "Point the Homebrew formula at $tag"
git push -q origin main
echo "Released $tag. Users update with: brew update && brew upgrade riga"
