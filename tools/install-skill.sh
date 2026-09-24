#!/bin/sh
# install-skill.sh: install the oxdm Agent Skill into an agent's
# skills directory, or flatten it into an AGENTS.md.
#
# Agent Skills (SKILL.md) is an open standard: the same skill files work
# across SKILL.md-compatible agents. Only the install *path* differs per
# tool (and per scope: global vs. this project), which is what this
# script handles.
set -eu

SKILL="oxdm"
FILES="SKILL.md reference.md examples.md"
# Canonical skill location is inside the plugin; .claude/skills/${SKILL} is a
# symlink to it for in-repo dogfooding. Use the real path so GitHub raw
# (which does not follow symlinks) works when this script is piped via curl.
SKILL_SUBPATH="plugins/${SKILL}/skills/${SKILL}"
RAW_BASE="https://raw.githubusercontent.com/jd1378/oxdm/main/${SKILL_SUBPATH}"

usage() {
  cat <<EOF
Usage: install-skill.sh [AGENT] [SCOPE]

AGENT (default: claude):
  claude | codex | copilot | gemini | cursor

SCOPE:
  --global        install for all projects (agent's home skills dir)
  --project       install for the current project only (./<agent dir>)
  --dir DIR       install into DIR/${SKILL} (any path; no detection)
  (none)          if run inside a project, ask global vs project
                  (falls back to --global when non-interactive)

Other:
  agents-md [FILE]  flatten the skill into FILE for non-SKILL.md agents
                    (default ./AGENTS.md)
  -h, --help

Known skills directories:
  claude   ~/.claude/skills            ./.claude/skills
  codex    ~/.codex/skills             ./.codex/skills
  copilot  (use --dir)                 ./.github/skills
  gemini   ~/.gemini/antigravity/skills (project: use --dir)
  cursor   (paths vary; use --dir)
The spec standardizes the file format, not the install path; if a tool
moved its skills directory, use '--dir DIR' with the correct location.
EOF
}

# --- skill source staging ---------------------------------------------------

# Stage skill files into \$STAGE, from a local checkout if present, else
# download them from GitHub (supports piping this script via curl).
stage() {
  STAGE=$(mktemp -d)
  script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  local_src="${script_dir}/../${SKILL_SUBPATH}"
  if [ -f "${local_src}/SKILL.md" ]; then
    for f in $FILES; do cp "${local_src}/${f}" "${STAGE}/${f}"; done
  else
    for f in $FILES; do curl -fsSL "${RAW_BASE}/${f}" -o "${STAGE}/${f}"; done
  fi
}

# --- path resolution --------------------------------------------------------

# Print the skills directory for "$1" agent at "$2" scope, or fail (1).
agent_dir() {
  case "$1:$2" in
    claude:global)  printf '%s' "${HOME}/.claude/skills" ;;
    claude:project) printf '%s' "./.claude/skills" ;;
    codex:global)   printf '%s' "${HOME}/.codex/skills" ;;
    codex:project)  printf '%s' "./.codex/skills" ;;
    copilot:project) printf '%s' "./.github/skills" ;;
    gemini:global)  printf '%s' "${HOME}/.gemini/antigravity/skills" ;;
    *) return 1 ;;
  esac
}

# Heuristic: does the current directory look like a project root?
project_detected() {
  for m in .git .claude .codex .cursor .github .agents \
           Cargo.toml package.json pyproject.toml go.mod; do
    [ -e "$m" ] && return 0
  done
  return 1
}

# True when we can prompt the user. Reads /dev/tty (not stdin), so this
# holds even when the script is piped via `curl | sh` in a terminal.
interactive() { [ -t 1 ] && [ -r /dev/tty ]; }

# Interactively choose the agent; sets AGENT.
prompt_agent() {
  {
    printf 'Install the %s skill for which agent?\n' "$SKILL"
    printf '  1) claude (default)\n  2) codex\n  3) copilot\n  4) gemini\n  5) cursor\n'
    printf 'Choose [1-5] (default 1): '
  } >/dev/tty
  read a </dev/tty 2>/dev/null || a=""
  case "$a" in
    2) AGENT=codex ;; 3) AGENT=copilot ;; 4) AGENT=gemini ;; 5) AGENT=cursor ;;
    *) AGENT=claude ;;
  esac
}

# Interactively ask global vs. project; sets SCOPE.
prompt_scope() {
  gp=$(agent_dir "$AGENT" global 2>/dev/null || echo "(unsupported; pick a dir)")
  pp=$(agent_dir "$AGENT" project 2>/dev/null || echo "(unsupported; pick a dir)")
  {
    printf 'Install scope for "%s":\n' "$AGENT"
    printf '  1) globally      %s\n' "$gp"
    printf '  2) this project  %s\n' "$pp"
    printf 'Choose [1/2] (default 1): '
  } >/dev/tty
  read ans </dev/tty 2>/dev/null || ans=""
  case "$ans" in 2) SCOPE=project ;; *) SCOPE=global ;; esac
}

# --- install actions --------------------------------------------------------

copy_to() {
  dir="$1"
  mkdir -p "$dir"
  for f in $FILES; do cp "${STAGE}/${f}" "${dir}/${f}"; done
  echo "Installed ${SKILL} -> ${dir}"
}

# Append (or replace) a self-contained skill block in an AGENTS.md, for
# agents that read always-on instructions rather than SKILL.md.
flatten_agents_md() {
  out="$1"
  begin="<!-- BEGIN ${SKILL} skill -->"
  end="<!-- END ${SKILL} skill -->"
  tmp=$(mktemp)
  if [ -f "$out" ]; then
    awk -v b="$begin" -v e="$end" '$0==b{skip=1} !skip{print} $0==e{skip=0}' "$out" > "$tmp"
  fi
  {
    echo "$begin"
    echo
    # SKILL.md body without its YAML frontmatter.
    awk 'NR==1&&$0=="---"{f=1;next} f&&$0=="---"{f=0;next} !f{print}' "${STAGE}/SKILL.md"
    echo; echo "---"; echo
    cat "${STAGE}/reference.md"
    echo; echo "---"; echo
    cat "${STAGE}/examples.md"
    echo
    echo "$end"
  } >> "$tmp"
  mv "$tmp" "$out"
  echo "Flattened ${SKILL} -> ${out}"
}

# --- argument parsing -------------------------------------------------------

AGENT=claude
AGENT_SET=""
SCOPE=""
DIR=""

# 'agents-md' is a distinct mode; handle it up front.
if [ "${1:-}" = "agents-md" ]; then
  stage; trap 'rm -rf "$STAGE"' EXIT
  flatten_agents_md "${2:-./AGENTS.md}"
  exit 0
fi

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help|help) usage; exit 0 ;;
    --global)  SCOPE=global ;;
    --project) SCOPE=project ;;
    --dir)     shift; DIR="${1:-}"; [ -n "$DIR" ] || { echo "error: --dir needs a path" >&2; exit 2; } ;;
    claude|codex|copilot|gemini|cursor) AGENT="$1"; AGENT_SET=1 ;;
    *) echo "error: unknown argument '$1'" >&2; usage; exit 2 ;;
  esac
  shift
done

stage
trap 'rm -rf "$STAGE"' EXIT

if [ -n "$DIR" ]; then
  copy_to "${DIR}/${SKILL}"
  exit 0
fi

# Choose the agent if not given. Interactive (incl. via curl | sh in a
# terminal) asks; otherwise defaults to claude.
if [ -z "$AGENT_SET" ] && interactive; then
  prompt_agent
fi

# Resolve scope: explicit flag wins; interactive asks; else default global
# (with a note when a project was detected).
if [ -z "$SCOPE" ]; then
  if interactive; then
    prompt_scope
  elif project_detected; then
    SCOPE=global
    echo "note: project detected but non-interactive; installing globally (use --project to override)" >&2
  else
    SCOPE=global
  fi
fi

# Resolve the target dir; offer an interactive fallback for combos with no
# known path (e.g. cursor, or gemini --project).
if ! target=$(agent_dir "$AGENT" "$SCOPE" 2>/dev/null); then
  if interactive; then
    printf 'No known %s skills path for "%s". Enter a target directory: ' "$SCOPE" "$AGENT" >/dev/tty
    read d </dev/tty 2>/dev/null || d=""
    [ -n "$d" ] || { echo "error: no directory given" >&2; exit 2; }
    copy_to "${d}/${SKILL}"
    exit 0
  fi
  echo "error: no known ${SCOPE} skills path for '${AGENT}'. Use --dir DIR." >&2
  exit 2
fi
copy_to "${target}/${SKILL}"
