#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
I18N_REPO="${I18N_REPO:-/projects/ai/greentic-ng/greentic-i18n}"
MODE="${1:-all}"
EN_PATH="$ROOT_DIR/crates/packc/i18n/en.json"

TARGET_LANGS=(
  ar ar-AE ar-DZ ar-EG ar-IQ ar-MA ar-SA ar-SD ar-SY ar-TN
  ay bg bn cs da de el en-GB es et fa fi fr gn gu hi hr ht hu
  id it ja km kn ko lo lt lv ml mr ms my nah ne nl no pa pl pt
  qu ro ru si sk sr sv ta te th tl tr uk ur vi zh
)

if [[ ! -x "$I18N_REPO/tools/i18n.sh" ]]; then
  echo "missing translator script: $I18N_REPO/tools/i18n.sh" >&2
  exit 1
fi

if [[ ! -f "$EN_PATH" ]]; then
  echo "missing English catalog: $EN_PATH" >&2
  exit 1
fi

# translator `--langs all` resolves from files next to EN_PATH; seed missing targets
I18N_DIR="$(dirname "$EN_PATH")"
for lang in "${TARGET_LANGS[@]}"; do
  lang_file="$I18N_DIR/$lang.json"
  if [[ ! -f "$lang_file" ]]; then
    printf "{\n}\n" > "$lang_file"
  fi
done

EN_PATH="$EN_PATH" "$I18N_REPO/tools/i18n.sh" "$MODE"
