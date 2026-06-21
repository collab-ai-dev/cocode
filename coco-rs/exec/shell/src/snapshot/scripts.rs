//! Shell-specific snapshot scripts.
//!
//! Each script captures the user's shell environment including:
//! - Functions (filtered: excludes `_`-prefixed completion funcs, keeps `__`-prefixed)
//! - Shell options
//! - Aliases (with size limits)
//! - Environment variables (filtered for valid names and exclusions)
//!
//! Scripts output a marker (`# Snapshot file`) followed by shell code
//! that can be sourced to restore the captured state.

/// Environment variables excluded from snapshots.
pub const EXCLUDED_EXPORT_VARS: &[&str] = &["PWD", "OLDPWD"];

/// Returns the regex alternation for excluded exports.
fn excluded_exports_regex() -> String {
    EXCLUDED_EXPORT_VARS.join("|")
}

/// Returns the zsh snapshot script.
///
/// Captures functions individually (filtered), shell options (setopt),
/// aliases, and exports. Uses `typeset -f` for function bodies.
pub fn zsh_snapshot_script() -> String {
    let excluded = excluded_exports_regex();
    let script = r##"if [[ -n "$ZDOTDIR" ]]; then
  rc="$ZDOTDIR/.zshrc"
else
  rc="$HOME/.zshrc"
fi
[[ -r "$rc" ]] && . "$rc" < /dev/null
print '# Snapshot file'
print '# Unset all aliases to avoid conflicts with functions'
print 'unalias -a 2>/dev/null || true'
print '# Functions'
# Force autoload all functions first
typeset -f > /dev/null 2>&1
# Capture functions individually, filtering out single-underscore completion
# functions (e.g. _git, _npm) but keeping double-underscore helpers.
typeset +f | grep -vE '^_[^_]' | while read func; do
  typeset -f "$func"
done
print ''
setopt_count=$(setopt | wc -l | tr -d ' ')
print "# setopts $setopt_count"
setopt | sed 's/^/setopt /' | head -n 1000
print ''
alias_count=$(alias -L | wc -l | tr -d ' ')
print "# aliases $alias_count"
alias -L | head -n 1000
print ''
export_lines=$(export -p | awk '
/^(export|declare -x|typeset -x) / {
  line=$0
  name=line
  sub(/^(export|declare -x|typeset -x) /, "", name)
  sub(/=.*/, "", name)
  if (name ~ /^(EXCLUDED_EXPORTS)$/) {
    next
  }
  if (name ~ /^[A-Za-z_][A-Za-z0-9_]*$/) {
    print line
  }
}')
export_count=$(printf '%s\n' "$export_lines" | sed '/^$/d' | wc -l | tr -d ' ')
print "# exports $export_count"
if [[ -n "$export_lines" ]]; then
  print -r -- "$export_lines"
fi
"##;
    script.replace("EXCLUDED_EXPORTS", &excluded)
}

/// Returns the bash snapshot script.
///
/// Captures functions via base64 encoding (preserves special chars in function
/// bodies), shell options (shopt + set -o), aliases with `--` prefix, and exports.
/// Enables `expand_aliases` explicitly for alias expansion after sourcing.
pub fn bash_snapshot_script() -> String {
    let excluded = excluded_exports_regex();
    let script = r##"if [ -z "$BASH_ENV" ] && [ -r "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc" < /dev/null
fi
echo '# Snapshot file'
echo '# Unset all aliases to avoid conflicts with functions'
unalias -a 2>/dev/null || true
echo '# Functions'
# Force autoload all functions first
declare -f > /dev/null 2>&1
# Capture functions individually with base64 encoding to handle special characters.
# Filter out single-underscore completion functions (e.g. _git, _npm) but keep
# double-underscore helpers (e.g. __pyenv_init, __zsh_like_cd from mise).
declare -F | cut -d' ' -f3 | grep -vE '^_[^_]' | while read func; do
  encoded_func=$(declare -f "$func" | base64)
  echo "eval \"\$(echo '$encoded_func' | base64 -d)\" > /dev/null 2>&1"
done
echo ''
echo '# Shell Options'
shopt -p | head -n 1000
set -o | awk '$2=="on"{print "set -o " $1}' | head -n 1000
echo 'shopt -s expand_aliases'
echo ''
alias_count=$(alias | wc -l | tr -d ' ')
echo "# aliases $alias_count"
alias | sed 's/^alias //g' | sed 's/^/alias -- /' | head -n 1000
echo ''
export_lines=$(export -p | awk '
/^(export|declare -x|typeset -x) / {
  line=$0
  name=line
  sub(/^(export|declare -x|typeset -x) /, "", name)
  sub(/=.*/, "", name)
  if (name ~ /^(EXCLUDED_EXPORTS)$/) {
    next
  }
  if (name ~ /^[A-Za-z_][A-Za-z0-9_]*$/) {
    print line
  }
}')
export_count=$(printf '%s\n' "$export_lines" | sed '/^$/d' | wc -l | tr -d ' ')
echo "# exports $export_count"
if [ -n "$export_lines" ]; then
  printf '%s\n' "$export_lines"
fi
"##;
    script.replace("EXCLUDED_EXPORTS", &excluded)
}

/// Returns the POSIX sh snapshot script.
///
/// Uses POSIX-compatible commands with fallbacks for systems that may not
/// support all features (e.g. `typeset` vs `declare`).
pub fn sh_snapshot_script() -> String {
    let excluded = excluded_exports_regex();
    let script = r##"if [ -n "$ENV" ] && [ -r "$ENV" ]; then
  . "$ENV" < /dev/null
fi
echo '# Snapshot file'
echo '# Unset all aliases to avoid conflicts with functions'
unalias -a 2>/dev/null || true
echo '# Functions'
if command -v typeset >/dev/null 2>&1; then
  typeset -f
elif command -v declare >/dev/null 2>&1; then
  declare -f
fi
echo ''
if set -o >/dev/null 2>&1; then
  sh_opts=$(set -o | awk '$2=="on"{print $1}')
  sh_opt_count=$(printf '%s\n' "$sh_opts" | sed '/^$/d' | wc -l | tr -d ' ')
  echo "# setopts $sh_opt_count"
  if [ -n "$sh_opts" ]; then
    printf 'set -o %s\n' $sh_opts
  fi
else
  echo '# setopts 0'
fi
echo ''
if alias >/dev/null 2>&1; then
  alias_count=$(alias | wc -l | tr -d ' ')
  echo "# aliases $alias_count"
  alias
  echo ''
else
  echo '# aliases 0'
fi
if export -p >/dev/null 2>&1; then
  export_lines=$(export -p | awk '
/^(export|declare -x|typeset -x) / {
  line=$0
  name=line
  sub(/^(export|declare -x|typeset -x) /, "", name)
  sub(/=.*/, "", name)
  if (name ~ /^(EXCLUDED_EXPORTS)$/) {
    next
  }
  if (name ~ /^[A-Za-z_][A-Za-z0-9_]*$/) {
    print line
  }
}')
  export_count=$(printf '%s\n' "$export_lines" | sed '/^$/d' | wc -l | tr -d ' ')
  echo "# exports $export_count"
  if [ -n "$export_lines" ]; then
    printf '%s\n' "$export_lines"
  fi
else
  export_count=$(env | sort | awk -F= '$1 ~ /^[A-Za-z_][A-Za-z0-9_]*$/ { count++ } END { print count }')
  echo "# exports $export_count"
  env | sort | while IFS='=' read -r key value; do
    case "$key" in
      ""|[0-9]*|*[!A-Za-z0-9_]*|EXCLUDED_EXPORTS) continue ;;
    esac
    escaped=$(printf "%s" "$value" | sed "s/'/'\"'\"'/g")
    printf "export %s='%s'\n" "$key" "$escaped"
  done
fi
"##;
    script.replace("EXCLUDED_EXPORTS", &excluded)
}

/// Returns the PowerShell snapshot script.
///
/// Limited support — captures functions, aliases, and environment variables.
pub fn powershell_snapshot_script() -> &'static str {
    r##"$ErrorActionPreference = 'Stop'
Write-Output '# Snapshot file'
Write-Output '# Unset all aliases to avoid conflicts with functions'
Write-Output 'Remove-Item Alias:* -ErrorAction SilentlyContinue'
Write-Output '# Functions'
Get-ChildItem Function: | ForEach-Object {
    "function {0} {{`n{1}`n}}" -f $_.Name, $_.Definition
}
Write-Output ''
$aliases = Get-Alias
Write-Output ("# aliases " + $aliases.Count)
$aliases | ForEach-Object {
    "Set-Alias -Name {0} -Value {1}" -f $_.Name, $_.Definition
}
Write-Output ''
$envVars = Get-ChildItem Env:
Write-Output ("# exports " + $envVars.Count)
$envVars | ForEach-Object {
    $escaped = $_.Value -replace "'", "''"
    "`$env:{0}='{1}'" -f $_.Name, $escaped
}
"##
}

#[cfg(test)]
#[path = "scripts.test.rs"]
mod tests;
