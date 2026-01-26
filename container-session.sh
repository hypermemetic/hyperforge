#!/bin/bash
# container-session.sh - Load hyperforge-lforge2 and dependencies into claude-container
# with optional session import

# Don't use set -e so we can capture and display errors
# set -e

CLAUDE_CONTAINER=~/dev/controlflow/juggernautlabs/claude-container/claude-container
CLAUDE_CONTAINER_CP=~/dev/controlflow/juggernautlabs/claude-container/claude-container-cp
SESSION_NAME="${1:-hyperforge-lforge2}"
CURRENT_SESSION_DIR=~/.claude/projects/-Users-shmendez-dev-controlflow-hypermemetic-hyperforge-lforge2
CURRENT_SESSION_ID="${2:-5534791e-38d7-4478-b668-05b40ff8215f}"

echo "=== Hyperforge Container Session Setup ==="
echo "Session name: $SESSION_NAME"
echo "Current session ID: $CURRENT_SESSION_ID"
echo ""

# Check if claude-container exists
if [[ ! -x "$CLAUDE_CONTAINER" ]]; then
    echo "Error: claude-container not found at $CLAUDE_CONTAINER"
    exit 1
fi

# Check if Docker is running
if ! docker info &>/dev/null; then
    echo "Error: Docker is not running."
    echo "Start it with: colima start"
    exit 1
fi
echo "Docker: OK"

# Step 1: Create the git session without running (--no-run)
echo "Step 1: Creating git session with dependencies..."
cd ~/dev/controlflow/hypermemetic/hyperforge-lforge2

if ! "$CLAUDE_CONTAINER" --git-session "$SESSION_NAME" --no-run; then
    echo ""
    echo "Error: Failed to create git session. See output above."
    echo "Make sure Docker/Colima is running: colima start"
    exit 1
fi

# Step 2: Copy current session transcript into the container's state volume
if [[ -f "$CURRENT_SESSION_DIR/$CURRENT_SESSION_ID.jsonl" ]]; then
    echo ""
    echo "Step 2: Importing current session transcript..."

    # Create the projects directory structure in the container state
    # The container expects: /home/developer/.claude/projects/<project-path>/
    CONTAINER_PROJECT_DIR="/home/developer/.claude/projects/-workspace-hyperforge-lforge2"

    # Copy the session transcript
    echo "  Copying transcript: $CURRENT_SESSION_ID.jsonl"
    "$CLAUDE_CONTAINER_CP" "$CURRENT_SESSION_DIR/$CURRENT_SESSION_ID.jsonl" \
        "$SESSION_NAME:/home/developer/.claude/projects/-workspace-hyperforge-lforge2/$CURRENT_SESSION_ID.jsonl"

    # Copy sessions-index.json if it exists
    if [[ -f "$CURRENT_SESSION_DIR/sessions-index.json" ]]; then
        echo "  Copying sessions-index.json"
        "$CLAUDE_CONTAINER_CP" "$CURRENT_SESSION_DIR/sessions-index.json" \
            "$SESSION_NAME:/home/developer/.claude/projects/-workspace-hyperforge-lforge2/sessions-index.json"
    fi

    echo "  Session data imported!"
else
    echo ""
    echo "Step 2: Skipping session import (transcript not found)"
fi

echo ""
echo "=== Setup Complete ==="
echo ""
echo "To start the container:"
echo "  $CLAUDE_CONTAINER --git-session $SESSION_NAME --continue"
echo ""
echo "Or without continuing previous conversation:"
echo "  $CLAUDE_CONTAINER --git-session $SESSION_NAME"
echo ""
echo "Inside container, workspace layout:"
echo "  /workspace/hyperforge-lforge2/  (main project)"
echo "  /workspace/hub-core/            (dependency)"
echo "  /workspace/hub-macro/           (dependency)"
echo "  /workspace/hub-transport/       (dependency)"
