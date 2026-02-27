#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
I18N_REPO="${I18N_REPO:-/projects/ai/greentic-ng/greentic-i18n}"
MODE="${1:-all}"
CORE_EN_PATH="$ROOT_DIR/crates/packc/i18n/en.json"
WIZARD_EN_GB_PATH="$ROOT_DIR/crates/packc/i18n/pack_wizard/en-GB.json"

TARGET_LANGS=(
  ar ar-AE ar-DZ ar-EG ar-IQ ar-MA ar-SA ar-SD ar-SY ar-TN
  ay bg bn cs da de el en-GB es et fa fi fr gn gu hi hr ht hu
  id it ja km kn ko lo lt lv ml mr ms my nah ne nl no pa pl pt
  qu ro ru si sk sr sv ta te th tl tr uk ur vi zh
)
WIZARD_TARGET_LOCALES=("${TARGET_LANGS[@]}" "fr-FR" "nl-NL")

if [[ ! -x "$I18N_REPO/tools/i18n.sh" ]]; then
  echo "missing translator script: $I18N_REPO/tools/i18n.sh" >&2
  exit 1
fi

if [[ ! -f "$CORE_EN_PATH" ]]; then
  echo "missing English catalog: $CORE_EN_PATH" >&2
  exit 1
fi

if [[ ! -f "$WIZARD_EN_GB_PATH" ]]; then
  echo "missing wizard English catalog: $WIZARD_EN_GB_PATH" >&2
  exit 1
fi

# translator `--langs all` resolves from files next to EN_PATH; seed missing targets
I18N_DIR="$(dirname "$CORE_EN_PATH")"
for lang in "${TARGET_LANGS[@]}"; do
  lang_file="$I18N_DIR/$lang.json"
  if [[ ! -f "$lang_file" ]]; then
    printf "{\n}\n" > "$lang_file"
  fi
done

WIZARD_I18N_DIR="$(dirname "$WIZARD_EN_GB_PATH")"
for locale in "${WIZARD_TARGET_LOCALES[@]}"; do
  locale_file="$WIZARD_I18N_DIR/$locale.json"
  if [[ ! -f "$locale_file" ]]; then
    printf "{\n}\n" > "$locale_file"
  fi
done

EN_PATH="$CORE_EN_PATH" CORE_EN_PATH="$CORE_EN_PATH" "$I18N_REPO/tools/i18n.sh" "$MODE"
EN_PATH="$WIZARD_EN_GB_PATH" CORE_EN_PATH="$WIZARD_EN_GB_PATH" "$I18N_REPO/tools/i18n.sh" "$MODE"
