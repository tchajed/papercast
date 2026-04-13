#!/usr/bin/env bash
#
# Sync secrets between .env and Fly.io.
#
# Usage:
#   ./scripts/sync-secrets.sh push   # Push .env secrets to Fly.io
#   ./scripts/sync-secrets.sh status # Show which secrets are set where
#
# Tigris credentials (AWS_*) are auto-managed by `fly storage create` and excluded from push.

set -euo pipefail

ENV_FILE="${ENV_FILE:-.env}"

# Secrets to push to Fly (API keys + app config, not Tigris or local-only settings)
SYNC_KEYS=(
    ANTHROPIC_API_KEY
    GOOGLE_API_KEY
    ADMIN_TOKEN
    PUBLIC_URL
)

# All keys relevant for status display
ALL_KEYS=(
    FLY_API_TOKEN
    DATABASE_URL
    AWS_ACCESS_KEY_ID
    AWS_SECRET_ACCESS_KEY
    AWS_ENDPOINT_URL_S3
    AWS_REGION
    BUCKET_NAME
    ANTHROPIC_API_KEY
    GOOGLE_API_KEY
    ADMIN_TOKEN
    PUBLIC_URL
)

read_env_value() {
    local key="$1"
    if [[ -f "$ENV_FILE" ]]; then
        grep -E "^${key}=" "$ENV_FILE" 2>/dev/null | head -1 | cut -d= -f2-
    fi
}

case "${1:-status}" in
    push)
        echo "Pushing secrets from $ENV_FILE to Fly.io..."
        args=()
        for key in "${SYNC_KEYS[@]}"; do
            val=$(read_env_value "$key")
            if [[ -n "$val" ]]; then
                args+=("${key}=${val}")
            else
                echo "  SKIP $key (not set in $ENV_FILE)"
            fi
        done
        if [[ ${#args[@]} -gt 0 ]]; then
            fly secrets set "${args[@]}"
            echo "Done. Pushed ${#args[@]} secrets."
        else
            echo "No secrets to push."
        fi
        ;;

    status)
        echo "Secret status:"
        echo ""
        printf "  %-25s  %-10s  %-10s\n" "KEY" "LOCAL" "FLY"
        printf "  %-25s  %-10s  %-10s\n" "---" "-----" "---"

        fly_secrets=""
        if command -v fly &>/dev/null; then
            fly_secrets=$(fly secrets list 2>/dev/null || true)
        fi

        for key in "${ALL_KEYS[@]}"; do
            local_val=$(read_env_value "$key")
            local_status="missing"
            if [[ -n "$local_val" ]]; then
                local_status="set"
            fi

            fly_status="—"
            if [[ "$key" == "FLY_API_TOKEN" ]]; then
                fly_status="n/a"
            elif [[ -n "$fly_secrets" ]]; then
                if echo "$fly_secrets" | grep -q "^$key"; then
                    fly_status="set"
                else
                    fly_status="missing"
                fi
            fi

            printf "  %-25s  %-10s  %-10s\n" "$key" "$local_status" "$fly_status"
        done
        ;;

    *)
        echo "Usage: $0 {push|status}"
        exit 1
        ;;
esac
