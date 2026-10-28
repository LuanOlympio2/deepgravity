#!/usr/bin/env bash
set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "🌌 Iniciando o DeepGravity..."

# Verifica se o backend universal-api está respondendo
if curl -s http://127.0.0.1:8000/v1/models >/dev/null 2>&1; then
    echo "✓ Backend API local ativo em 127.0.0.1:8000 (Fahremback/deepseek-web-universal-api)"
else
    echo "⚠️  Aviso: Backend local não detectado em 127.0.0.1:8000."
    echo "   Para rodar o modelo gratuito, inicie a dependência do backend:"
    echo "   git clone https://github.com/Fahremback/deepseek-web-universal-api.git"
    echo "   cd deepseek-web-universal-api && python3 -m venv .venv && ./.venv/bin/pip install -r requirements.txt"
    echo "   ./.venv/bin/python openai_server.py --host 127.0.0.1 --port 8000"
    echo ""
fi

# Executa o DeepGravity
if [ -f "$DIR/target/release/deepgravity" ]; then
    exec "$DIR/target/release/deepgravity" --gui "$@"
else
    echo "Compilando binário release pela primeira vez..."
    cargo build --release --manifest-path "$DIR/Cargo.toml"
    exec "$DIR/target/release/deepgravity" --gui "$@"
fi
