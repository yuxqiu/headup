#!/usr/bin/env bash
# Regenerate the spoken prompts in assets/voice/ with Piper and the en_GB "alba" voice (CC BY 4.0).
# The phrases must match `Phrase` in src/core/cues.rs. Usage: tools/gen_voice.sh
set -euo pipefail
cd "$(dirname "$0")/.."
model=research/voices/en_GB-alba-medium.onnx
base=https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/alba/medium/en_GB-alba-medium.onnx
mkdir -p research/voices assets/voice
[ -f "$model" ] || curl -sSfL -o "$model" "$base"
[ -f "$model.json" ] || curl -sSfL -o "$model.json" "$base.json"
declare -A phrases=(
  [sit_upright]="Sit upright and look straight ahead."
  [calibrated]="Calibrated. Monitoring your posture."
  [moved]="You moved. Hold still, looking straight ahead."
  [lift_head]="Gently lift your head."
)
for name in "${!phrases[@]}"; do
  echo "${phrases[$name]}" | nix shell nixpkgs#piper-tts -c piper -m "$model" -f "assets/voice/$name.wav"
done
ls -l assets/voice
