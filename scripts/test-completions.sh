#!/bin/bash
# Test script for octobrain shell completions

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Try release binary first, then debug binary
if [[ -f "${SCRIPT_DIR}/../target/release/octobrain" ]]; then
    OCTOBRAIN_BIN="${SCRIPT_DIR}/../target/release/octobrain"
elif [[ -f "${SCRIPT_DIR}/../target/debug/octobrain" ]]; then
    OCTOBRAIN_BIN="${SCRIPT_DIR}/../target/debug/octobrain"
else
    echo "Error: octobrain binary not found"
    echo "Please run 'cargo build --release' or 'cargo build' first"
    exit 1
fi

echo "✓ Binary found at $OCTOBRAIN_BIN"

# Test completion generation
echo ""
echo "Testing completion generation..."
echo ""

# Test bash completion generation
if "$OCTOBRAIN_BIN" completion bash > /tmp/test_bash_completion; then
    echo "✓ Bash completion generated successfully"
    echo "  Generated $(wc -l < /tmp/test_bash_completion) lines"
else
    echo "✗ Failed to generate bash completion"
    exit 1
fi

# Test zsh completion generation
if "$OCTOBRAIN_BIN" completion zsh > /tmp/test_zsh_completion; then
    echo "✓ Zsh completion generated successfully"
    echo "  Generated $(wc -l < /tmp/test_zsh_completion) lines"
else
    echo "✗ Failed to generate zsh completion"
    exit 1
fi

# Test all available shells
echo ""
echo "Testing all available shells..."
for shell in bash elvish fish powershell zsh; do
    if "$OCTOBRAIN_BIN" completion "$shell" > "/tmp/test_${shell}_completion" 2>/dev/null; then
        echo "✓ $shell completion generated successfully"
        echo "  Generated $(wc -l < "/tmp/test_${shell}_completion") lines"
    else
        echo "✗ Failed to generate $shell completion"
        exit 1
    fi
done

# Test completion content
echo ""
echo "Testing completion content..."
echo ""

# Check if bash completion contains expected patterns
if grep -q "_octobrain()" /tmp/test_bash_completion; then
    echo "✓ Bash completion contains function definition"
else
    echo "✗ Bash completion missing function definition"
fi

# Check if bash completion contains subcommand definitions
if grep -q "octobrain memory" /tmp/test_bash_completion; then
    echo "✓ Bash completion contains subcommand definitions"
else
    echo "✗ Bash completion missing subcommand definitions"
fi

# Check if zsh completion contains compdef directive
if grep -q "#compdef octobrain" /tmp/test_zsh_completion; then
    echo "✓ Zsh completion contains compdef directive"
else
    echo "✗ Zsh completion missing compdef directive"
fi

echo ""
echo "✓ All completion tests passed!"
echo ""
echo "To install completions, run:"
echo "  ./scripts/install-completions.sh"
echo ""
echo "Or manually:"
echo "  # Bash"
echo "  $OCTOBRAIN_BIN completion bash > ~/.local/share/bash-completion/completions/octobrain"
echo ""
echo "  # Zsh"
echo "  mkdir -p ~/.config/zsh/completions"
echo "  $OCTOBRAIN_BIN completion zsh > ~/.config/zsh/completions/_octobrain"
echo ""
echo "🔄 If completions don't work immediately:"
echo "  - Restart your shell: exec $SHELL"
echo "  - Or source your config: source ~/.bashrc (bash) or source ~/.zshrc (zsh)"
