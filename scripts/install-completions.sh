#!/bin/bash
# Installation script for octobrain shell completions

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

echo "Installing shell completions for octobrain..."

# Detect shell and install accordingly
detect_shell() {
    if [[ -n "$ZSH_VERSION" ]]; then
        echo "zsh"
    elif [[ -n "$BASH_VERSION" ]]; then
        echo "bash"
    else
        # Try to detect from SHELL environment variable
        case "$SHELL" in
            */zsh)
                echo "zsh"
                ;;
            */bash)
                echo "bash"
                ;;
            *)
                echo "unknown"
                ;;
        esac
    fi
}

SHELL_TYPE=$(detect_shell)

# Install bash completion
install_bash_completion() {
    echo "Installing bash completion..."
    
    # Standard bash completion directories (in order of preference)
    BASH_COMPLETION_DIRS=(
        "$HOME/.local/share/bash-completion/completions"
        "$HOME/.bash_completion.d"
        "/usr/local/etc/bash_completion.d"
    )
    
    # Find first writable directory
    BASH_DIR=""
    for dir in "${BASH_COMPLETION_DIRS[@]}"; do
        if [[ -d "$(dirname "$dir")" ]] && [[ -w "$(dirname "$dir")" ]]; then
            BASH_DIR="$dir"
            break
        fi
    done
    
    # Create user directory as fallback
    if [[ -z "$BASH_DIR" ]]; then
        BASH_DIR="$HOME/.local/share/bash-completion/completions"
        mkdir -p "$BASH_DIR"
    fi
    
    # Generate completion
    "$OCTOBRAIN_BIN" completion bash > "$BASH_DIR/octobrain"
    
    echo "✓ Bash completion installed to $BASH_DIR/octobrain"
    
    # Check if bash-completion is properly configured
    if ! grep -q "bash-completion" "$HOME/.bashrc" 2>/dev/null && \
       ! grep -q "bash-completion" "$HOME/.bash_profile" 2>/dev/null; then
        echo ""
        echo "📝 To enable bash completion, add this to your ~/.bashrc:"
        echo "   # Enable bash completion"
        echo "   if [[ -f /usr/share/bash-completion/bash_completion ]]; then"
        echo "       source /usr/share/bash-completion/bash_completion"
        echo "   elif [[ -f /usr/local/etc/bash_completion ]]; then"
        echo "       source /usr/local/etc/bash_completion"
        echo "   fi"
        echo ""
        echo "Alternatively, add this line to regenerate completions:"
        echo "   autoload -U compinit && compinit -d ~/.zcompdump"
    fi
}

# Install zsh completion
install_zsh_completion() {
    echo "Installing zsh completion..."
    
    # Standard zsh completion directories (in order of preference)
    ZSH_COMPLETION_DIRS=(
        "$HOME/.local/share/zsh/site-functions"
        "$HOME/.zsh/completions"
        "$HOME/.config/zsh/completions"
        "/usr/local/share/zsh/site-functions"
        "/usr/share/zsh/site-functions"
    )
    
    # Find first writable directory
    ZSH_DIR=""
    for dir in "${ZSH_COMPLETION_DIRS[@]}"; do
        if [[ -d "$(dirname "$dir")" ]] && [[ -w "$(dirname "$dir")" ]]; then
            ZSH_DIR="$dir"
            break
        fi
    done
    
    # Create user directory as fallback
    if [[ -z "$ZSH_DIR" ]]; then
        ZSH_DIR="$HOME/.local/share/zsh/site-functions"
        mkdir -p "$ZSH_DIR"
    fi
    
    # Generate completion
    "$OCTOBRAIN_BIN" completion zsh > "$ZSH_DIR/_octobrain"
    
    echo "✓ Zsh completion installed to $ZSH_DIR/_octobrain"
    
    # Check if directory is in fpath
    if ! grep -q "fpath=($ZSH_DIR)" "$HOME/.zshrc" 2>/dev/null; then
        echo ""
        echo "📝 To enable zsh completion, ensure your ~/.zshrc contains:"
        echo "   # Add completion directory to fpath"
        echo "   fpath=($ZSH_DIR)"
        echo ""
        echo "Alternatively, add these lines to regenerate completions:"
        echo "   autoload -U compinit && compinit -d ~/.zcompdump"
        echo "   exec zsh  # to restart your shell"
    fi
}

# Main installation logic
case "$1" in
    bash)
        install_bash_completion
        ;;
    zsh)
        install_zsh_completion
        ;;
    *)
        echo "Usage: $0 [bash|zsh|both]"
        echo "  bash - Install bash completion only"
        echo "  zsh  - Install zsh completion only"
        echo "  both - Install both completions (default)"
        exit 1
        ;;
esac

echo ""
echo "✅ Shell completion installation complete!"
echo ""
echo "🔄 If completions don't work immediately:"
echo "   - Restart your shell: exec $SHELL"
echo "   - Or source your config: source ~/.bashrc (bash) or source ~/.zshrc (zsh)"
