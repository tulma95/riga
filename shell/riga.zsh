# Riga's optional iTerm2 integration. Source after other ZLE customizations.
# Settings (set before sourcing): RIGA_BIN, RIGA_SOCKET, RIGA_AUTOSTART, RIGA_KEY_NEXT,
# RIGA_KEY_PREV, RIGA_KEY_ACCEPT, RIGA_KEY_DISMISS. Keys use bindkey syntax.
[[ -o interactive && ${TERM_PROGRAM-} == iTerm.app && -n ${ITERM_SESSION_ID-} ]] || return 0
(( ${+_RIGA_LOADED} )) && return 0
zmodload zsh/net/socket || return 0
zmodload zsh/system || return 0
zmodload zsh/datetime || return 0
autoload -Uz add-zle-hook-widget

typeset -g _RIGA_LOADED=1 _RIGA_FD='' _RIGA_RX='' _RIGA_BUFFER='' _RIGA_PWD=''
typeset -g _RIGA_PATH='' _RIGA_ENCODED_PATH=''
typeset -g _RIGA_SESSION="${$}-${RANDOM}-${EPOCHREALTIME}"
typeset -gi _RIGA_REV=0 _RIGA_CURSOR=-1 _RIGA_EDITING=0 _RIGA_VISIBLE=0
typeset -gA _RIGA_OLD_WIDGET _RIGA_OLD_MACRO _RIGA_BINDINGS
_RIGA_OLD_WIDGET=()
_RIGA_OLD_MACRO=()
_RIGA_BINDINGS=()
: ${RIGA_BIN:=riga} ${RIGA_SOCKET:=/tmp/riga-${UID}/socket}
: ${RIGA_KEY_NEXT:='^N'} ${RIGA_KEY_PREV:='^P'} ${RIGA_KEY_ACCEPT:='^I'} ${RIGA_KEY_DISMISS:='^G'}
: ${RIGA_AUTOSTART:=1}

# Start the server when none answers, detached so loading never waits. Started
# from iTerm2, it gets iTerm2's permissions. Concurrent starts exit on its lock.
[[ $RIGA_AUTOSTART == 1 ]] && () {
  local REPLY
  if zsocket "$RIGA_SOCKET" 2>/dev/null; then
    exec {REPLY}>&-
  else
    nohup "$RIGA_BIN" serve </dev/null >>"${${TMPDIR:-/tmp}%/}/riga-serve.log" 2>&1 &!
  fi
}

_riga_encode() {
  REPLY=${1//\%/%25}
  REPLY=${REPLY//$'\t'/%09}
  REPLY=${REPLY//$'\n'/%0A}
  REPLY=${REPLY//$'\r'/%0D}
}
_riga_decode() {
  REPLY=${1//\%09/$'\t'}
  REPLY=${REPLY//\%0A/$'\n'}
  REPLY=${REPLY//\%0D/$'\r'}
  REPLY=${REPLY//\%25/%}
}
_riga_disconnect() {
  if [[ -n $_RIGA_FD ]]; then
    zle -F "$_RIGA_FD" 2>/dev/null
    exec {_RIGA_FD}>&-
  fi
  _RIGA_FD='' _RIGA_RX='' _RIGA_VISIBLE=0
}
_riga_send() {
  emulate -L zsh
  [[ -n $_RIGA_FD ]] || return 1
  local LC_ALL=C payload="$1"$'\n'
  local -i written=0
  # The descriptor is nonblocking. Backpressure drops this connection instead
  # of ever holding up typing; the next line-init will reconnect.
  (( ${#payload} <= 65536 )) || { _riga_disconnect; return 1; }
  syswrite -c written -o "$_RIGA_FD" "$payload" 2>/dev/null || { _riga_disconnect; return 1; }
  (( written == ${#payload} )) || { _riga_disconnect; return 1; }
}
_riga_connect() {
  [[ -n $_RIGA_FD ]] && return 0
  [[ -S $RIGA_SOCKET ]] || return 1
  zsocket "$RIGA_SOCKET" 2>/dev/null || return 1
  _RIGA_FD=$REPLY
  # One short helper at connection time; no subprocesses on input changes.
  "$RIGA_BIN" nonblock "$_RIGA_FD" 2>/dev/null || { _riga_disconnect; return 1; }
  zle -F -w "$_RIGA_FD" riga-read
  # The popup footer shows these bindings.
  local REPLY k keys=K
  for k in "$RIGA_KEY_NEXT" "$RIGA_KEY_PREV" "$RIGA_KEY_ACCEPT" "$RIGA_KEY_DISMISS"; do
    _riga_encode "$k"; keys+=$'\t'$REPLY
  done
  _riga_send "$keys"
}
_riga_publish() {
  emulate -L zsh
  (( _RIGA_EDITING )) && [[ -n $_RIGA_FD ]] || return 0
  [[ $BUFFER == $_RIGA_BUFFER && $CURSOR == $_RIGA_CURSOR && $PWD == $_RIGA_PWD && $PATH == $_RIGA_PATH ]] && return 0
  (( ++_RIGA_REV ))
  _RIGA_VISIBLE=0 _RIGA_BUFFER=$BUFFER _RIGA_CURSOR=$CURSOR _RIGA_PWD=$PWD
  # Avoid pathological buffers filling IPC or delaying the editor.
  (( ${#BUFFER} <= 16384 )) || { _riga_send $'E\t'"$_RIGA_SESSION"$'\t'"$_RIGA_REV"; return 0; }
  local encoded_pwd encoded_buffer REPLY
  if [[ $PATH != $_RIGA_PATH ]]; then
    _riga_encode "$PATH"
    _RIGA_PATH=$PATH _RIGA_ENCODED_PATH=$REPLY
  fi
  _riga_encode "$PWD"; encoded_pwd=$REPLY
  _riga_encode "$BUFFER"; encoded_buffer=$REPLY
  _riga_send $'S\t'"$_RIGA_SESSION"$'\t'"$ITERM_SESSION_ID"$'\t'"$_RIGA_REV"$'\t'"$CURSOR"$'\t'"$encoded_pwd"$'\t'"$encoded_buffer"$'\t'"$_RIGA_ENCODED_PATH"
  return 0
}
_riga_line_init() {
  _RIGA_EDITING=1 _RIGA_CURSOR=-1 _RIGA_VISIBLE=0
  _riga_connect && _riga_publish
  return 0
}
_riga_line_finish() {
  _RIGA_EDITING=0 _RIGA_VISIBLE=0
  (( ++_RIGA_REV ))
  _riga_send $'E\t'"$_RIGA_SESSION"$'\t'"$_RIGA_REV"
  return 0
}
_riga_reply() {
  emulate -L zsh
  local -a fields
  fields=("${(@ps:\t:)1}")
  (( _RIGA_EDITING )) || return 0
  [[ ${fields[2]-} == $_RIGA_REV && $BUFFER == $_RIGA_BUFFER && $CURSOR == $_RIGA_CURSOR && $PWD == $_RIGA_PWD && $PATH == $_RIGA_PATH ]] || return 0
  case ${fields[1]-} in
    P)
      [[ ${fields[3]-} == <-> ]] || return 0
      _RIGA_VISIBLE=$(( fields[3] > 0 ))
      ;;
    I)
      [[ ${fields[3]-} == <-> && ${fields[4]-} == <-> && ${#fields} == 5 ]] || return 0
      local -i start=${fields[3]} end=${fields[4]}
      (( start <= end && end <= ${#BUFFER} )) || return 0
      local REPLY prefix='' suffix=''
      _riga_decode "${fields[5]}"
      (( start > 0 )) && prefix=${BUFFER[1,start]}
      (( end < ${#BUFFER} )) && suffix=${BUFFER[end+1,-1]}
      BUFFER=$prefix$REPLY$suffix
      CURSOR=$(( start + ${#REPLY} ))
      _RIGA_VISIBLE=0
      _riga_publish
      zle -R
      ;;
  esac
}
_riga_read() {
  emulate -L zsh
  [[ -z ${2-} ]] || { _riga_disconnect; return 0; }
  local chunk line
  sysread -i "$_RIGA_FD" -s 65536 -t 0 chunk 2>/dev/null
  local -i result=$?
  (( result == 5 )) && return 0
  (( result == 0 )) || { _riga_disconnect; return 0; }
  _RIGA_RX+=$chunk
  (( ${#_RIGA_RX} <= 131072 )) || { _riga_disconnect; return 0; }
  while [[ $_RIGA_RX == *$'\n'* ]]; do
    line=${_RIGA_RX%%$'\n'*}
    _RIGA_RX=${_RIGA_RX#*$'\n'}
    _riga_reply "$line"
  done
}
_riga_key() {
  emulate -L zsh
  local action=${WIDGET##*-}
  # ZLE can invoke a key before pre-redraw; publish first so stale suggestions
  # never intercept a key for a buffer that has already changed.
  _riga_publish
  if (( _RIGA_VISIBLE )) && [[ -n $_RIGA_FD ]]; then
    _riga_send $'A\t'"$_RIGA_SESSION"$'\t'"$_RIGA_REV"$'\t'"$action" && {
      [[ $action == dismiss || $action == accept ]] && _RIGA_VISIBLE=0
      return 0
    }
  fi
  if (( ${+_RIGA_OLD_MACRO[$WIDGET]} )); then
    zle -U -- "${_RIGA_OLD_MACRO[$WIDGET]}"
  elif [[ -n ${_RIGA_OLD_WIDGET[$WIDGET]-} ]]; then
    zle "${_RIGA_OLD_WIDGET[$WIDGET]}"
  fi
}
_riga_bind() {
  emulate -L zsh
  local map=$1 action=$2 key=$3 widget="riga-${1}-${2}" original saved
  local -a parts
  saved=$(bindkey -L -M "$map" "$key") || return 0
  _RIGA_BINDINGS[$widget]=$saved
  parts=(${(z)saved})
  original=${(Q)parts[-1]}
  if [[ ${parts[2]-} == -s ]]; then
    _RIGA_OLD_MACRO[$widget]=$original
  else
    zle -A "$original" "${widget}-original" 2>/dev/null || return 0
    _RIGA_OLD_WIDGET[$widget]="${widget}-original"
  fi
  zle -N "$widget" _riga_key
  bindkey -M "$map" "$key" "$widget"
}
# Remove hooks and restore the exact original bindings in this shell.
riga-unload() {
  _riga_line_finish
  _riga_disconnect
  add-zle-hook-widget -d line-init _riga_line_init
  add-zle-hook-widget -d line-pre-redraw _riga_publish
  add-zle-hook-widget -d line-finish _riga_line_finish
  local widget
  for widget in ${(k)_RIGA_BINDINGS}; do
    eval "${_RIGA_BINDINGS[$widget]}"
    zle -D "$widget" "${widget}-original" 2>/dev/null
  done
  unset _RIGA_LOADED
}
zle -N riga-read _riga_read
add-zle-hook-widget line-init _riga_line_init
add-zle-hook-widget line-pre-redraw _riga_publish
add-zle-hook-widget line-finish _riga_line_finish
for _riga_map in emacs viins; do
  _riga_bind "$_riga_map" next "$RIGA_KEY_NEXT"
  _riga_bind "$_riga_map" prev "$RIGA_KEY_PREV"
  _riga_bind "$_riga_map" accept "$RIGA_KEY_ACCEPT"
  _riga_bind "$_riga_map" dismiss "$RIGA_KEY_DISMISS"
done
unset _riga_map
