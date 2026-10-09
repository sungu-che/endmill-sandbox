#!/usr/bin/env bash
# 리눅스에서 endmill_sandbox 라이브러리 테스트(tests/*.rs)를 백엔드별로 실행합니다.
#
#   ./test_linux.sh [cpu|vulkan|rocm|cuda] [cargo test 추가 인자...]
#   예) ./test_linux.sh vulkan                       # 전체 스위트 + tests/vulkan_backend.rs
#       ./test_linux.sh vulkan --test vulkan_backend # Vulkan 대조 테스트만
#
# 필요 패키지(Ubuntu 24.04 기준): protobuf-compiler (LanceDB), libvulkan1 mesa-vulkan-drivers (Vulkan)
# ROCm 은 /opt/rocm (또는 ROCM_PATH/HIP_PATH) 이 필요합니다.
set -euo pipefail
cd "$(dirname "$0")"

BACKEND="${1:-vulkan}"
[ $# -gt 0 ] && shift
case "$BACKEND" in
  cpu) FEATURE_ARGS=() ;;
  cuda|vulkan|rocm) FEATURE_ARGS=(--no-default-features --features "$BACKEND") ;;
  *) echo "unknown backend: $BACKEND (cpu|vulkan|rocm|cuda)"; exit 1 ;;
esac

if [ -z "${PROTOC:-}" ] && ! command -v protoc >/dev/null 2>&1; then
  echo "[SETUP] protoc 가 없습니다. 'sudo apt install protobuf-compiler' 또는 PROTOC=<경로> 를 지정하세요."; exit 1
fi

# GPU 가 없는 머신에서는 Mesa llvmpipe(lavapipe) 로 Vulkan 을 검증합니다.
# (포크는 기본적으로 CPU 타입 Vulkan 디바이스를 건너뜀. 실제 GPU 가 있으면 그것이 0번으로 선택됨)
if [ "$BACKEND" = vulkan ]; then export CANDLE_VULKAN_ALLOW_CPU="${CANDLE_VULKAN_ALLOW_CPU:-1}"; fi

# 앱 데이터 디렉터리를 오염시키지 않도록 격리
XDG_DATA_HOME="$(mktemp -d)"; export XDG_DATA_HOME
trap 'rm -rf "$XDG_DATA_HOME"' EXIT

cargo test "${FEATURE_ARGS[@]}" "$@"
