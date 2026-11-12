#!/usr/bin/env bash
# Build the component and write the release's two files into dist/:
#
#   dist/knobas_adapter_xwiki.wasm   the component
#   dist/knobas-adapter.json         the manifest knobas installs from
#
# The manifest names the artifact by file name and SHA-256, the interface
# version the component is built against, the hosts it may reach and the
# limits it asks for. knobas refuses an install whose hash, version or
# descriptor disagrees with it.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dist="$root/dist"
artifact=knobas_adapter_xwiki.wasm

cargo rustc --manifest-path "$root/Cargo.toml" -p knobas-adapter-xwiki \
    --target wasm32-wasip2 --release --crate-type cdylib --quiet

mkdir -p "$dist"
cp "$root/target/wasm32-wasip2/release/$artifact" "$dist/$artifact"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
if command -v sha256sum >/dev/null; then
    sha=$(sha256sum "$dist/$artifact" | cut -d' ' -f1)
else
    sha=$(shasum -a 256 "$dist/$artifact" | cut -d' ' -f1)
fi

cat > "$dist/knobas-adapter.json" <<EOF
{
  "adapter_kind": "xwiki",
  "adapter_version": "$version",
  "interface_version": "1.5.0",
  "artifact": { "file": "$artifact", "sha256": "$sha" },
  "hosts": ["{base_url}"],
  "limits": { "deadline_secs": 900, "memory_mib": 256 }
}
EOF

echo "dist/$artifact  $(wc -c < "$dist/$artifact" | tr -d ' ') bytes  sha256 $sha"
echo "dist/knobas-adapter.json  xwiki $version on knobas:adapter@1.5.0"
