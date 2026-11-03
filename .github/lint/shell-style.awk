function report(line, msg) {
  printf "%s:%d: %s\n", cur_file, line, msg
  bad = 1
}

function report_path(path, msg) {
  printf "%s: %s\n", path, msg
  bad = 1
}

function pop(stack) {
  return substr(stack, 1, length(stack) - 1)
}

function scan(text, mode, out, i, n, c, stack, top) {
  out = ""
  stack = ""
  n = length(text)
  for (i = 1; i <= n; i++) {
    c = substr(text, i, 1)
    top = substr(stack, length(stack), 1)
    if (top == "'") {
      if (c == "'") {
        stack = pop(stack)
        out = out c
      } else if (mode == "code") {
        out = out "_"
      }
      continue
    }
    if (c == "\\") {
      i++
      out = out "__"
      continue
    }
    if (top == "\"") {
      if (c == "\"") {
        stack = pop(stack)
        out = out c
      } else if (c == "$" && substr(text, i + 1, 1) == "(") {
        stack = stack "("
        out = out "$("
        i++
      } else if (mode == "code") {
        out = out "_"
      } else {
        out = out c
      }
      continue
    }
    if (top == "(" && c == ")") {
      stack = pop(stack)
    } else if (top == "(" && c == "(") {
      stack = stack "("
    } else if (c == "'" || c == "\"") {
      stack = stack c
    }
    out = out c
  }
  return out
}

function drop_subst(text, out, i, n, c, depth) {
  out = ""
  depth = 0
  n = length(text)
  for (i = 1; i <= n; i++) {
    c = substr(text, i, 1)
    if (c == "$" && substr(text, i + 1, 1) == "(") {
      depth++
      i++
    } else if (depth > 0 && c == "(") {
      depth++
    } else if (depth > 0 && c == ")") {
      depth--
    } else if (depth == 0) {
      out = out c
    }
  }
  return out
}

function trim(text) {
  sub(/^[[:space:]]+/, "", text)
  sub(/[[:space:]]+$/, "", text)
  return text
}

function first_word(text) {
  text = trim(text)
  while (match(text, /^[A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*[[:space:]]+/)) {
    text = substr(text, RLENGTH + 1)
  }
  sub(/^(!|time)[[:space:]]+/, "", text)
  sub(/[[:space:];&|<>()].*$/, "", text)
  return text
}

function split_steps(code, text, starts, ends, n, i, c, from, depth) {
  n = 0
  from = 1
  depth = 0
  for (i = 1; i <= length(code); i++) {
    c = substr(code, i, 1)
    if (c == "$" && substr(code, i + 1, 1) == "(") {
      depth++
      i++
    } else if (depth > 0 && c == ")") {
      depth--
    } else if (depth == 0 && c == ";") {
      n++
      starts[n] = from
      ends[n] = i - 1
      from = i + 1
    }
  }
  n++
  starts[n] = from
  ends[n] = length(code)
  return n
}

function note_conditions(step, line, parts, count, i, word) {
  step = drop_subst(step)
  if (match(step, /^[[:space:]]*(if|elif|while|until|!)[[:space:]]+/)) {
    word = first_word(substr(step, RLENGTH + 1))
    if (word != "") {
      calls[word] = line
    }
  }
  gsub(/&&|\|\|/, "\001", step)
  count = split(step, parts, "\001")
  for (i = 1; i < count; i++) {
    word = parts[i]
    sub(/^[[:space:]]*(if|elif|while|until)[[:space:]]+/, "", word)
    word = first_word(word)
    if (word != "") {
      calls[word] = line
    }
  }
}

function note_step(step, line, plain, name, rest, words, count, i, flat) {
  plain = trim(drop_subst(step))
  sub(/^[^[:space:]()]*\)[[:space:]]*/, "", plain)
  if (plain == "" || plain ~ /^(then|do|else|fi|done|esac|\{|\}|;|\)|\(|in)$/) {
    return
  }
  if (plain ~ /^(if|elif|while|until|for|case|select|function)[[:space:]]/ || plain ~ /^[^[:space:]]*\)$/) {
    return
  }
  if (plain ~ /^(local|declare|typeset|readonly)([[:space:]]|$)/) {
    count = split(plain, words, /[[:space:]]+/)
    for (i = 2; i <= count; i++) {
      name = words[i]
      sub(/[+]?=.*$/, "", name)
      if (name ~ /^[A-Za-z_][A-Za-z0-9_]*$/) {
        locals[name] = 1
      }
    }
    if (step !~ /\$\(/) {
      return
    }
  }
  if (match(plain, /^for[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/)) {
    return
  }
  if (match(plain, /^[A-Za-z_][A-Za-z0-9_]*[+]?=/)) {
    name = substr(plain, 1, RLENGTH)
    sub(/[+]?=$/, "", name)
    rest = substr(plain, RLENGTH + 1)
    if (rest !~ /[[:space:]]/ && name ~ /^[a-z_][a-z0-9_]*$/ && !(name in locals)) {
      report(line, "declare " name " with local inside the function (SH-11)")
    }
    if (rest !~ /[[:space:]]/ && step !~ /\$\(/) {
      return
    }
  }
  if (plain ~ /^(return|exit|break|continue|true|:)([[:space:]]|$)/) {
    return
  }
  body_count[cur_fn]++
  flat = cur_fn SUBSEP body_count[cur_fn]
  body_line[flat] = line
  body_safe[flat] = plain ~ /\|\|[[:space:]]*(return|exit|die|\{)([[:space:]]|$)/
}

function check_names(code, line, parts, count, i, words, total, j, word, declaring, name) {
  gsub(/\$\(|[;&|(){}]/, "\001", code)
  count = split(code, parts, "\001")
  for (i = 1; i <= count; i++) {
    total = split(trim(parts[i]), words, /[[:space:]]+/)
    declaring = 0
    for (j = 1; j <= total; j++) {
      word = words[j]
      if (!declaring && word ~ /^(then|do|else|elif|if|while|until|!|time)$/) {
        continue
      }
      if (!declaring && word ~ /^(local|declare|typeset|readonly|export)$/) {
        declaring = 1
        continue
      }
      if (declaring && word ~ /^-/) {
        continue
      }
      if (!match(word, /^[A-Za-z_][A-Za-z0-9_]*([+]?=|$)/)) {
        break
      }
      name = word
      sub(/[+]?=.*$/, "", name)
      if (!declaring && name == word) {
        break
      }
      if (name !~ /^[a-z_][a-z0-9_]*$/ && name !~ /^[A-Z_][A-Z0-9_]*$/) {
        report(line, "variable " name " is lower_snake_case, or UPPER_CASE when exported or constant (SH-11)")
      }
    }
  }
}

function check(text, line, code, expand, rest, name, steps, starts, ends, count, i, step) {
  code = scan(text, "code")
  expand = scan(text, "expand")
  if (match(expand, /(^|[;&|({}!][[:space:]]*|[[:space:]])echo([[:space:]]|$)/)) {
    if (substr(expand, RSTART + RLENGTH) ~ /\$/) {
      report(line, "print anything with an expansion through printf, not echo (SH-7)")
    }
  }
  if (match(code, /(^|[;&|({}!`][[:space:]]*|[[:space:]]|\$\()curl[[:space:]]/)) {
    rest = substr(text, RSTART + RLENGTH)
    if (rest !~ /\[@\]/) {
      if (rest !~ /(^|[[:space:]])(-[a-zA-Z]*f[a-zA-Z]*|--fail|--fail-with-body)([[:space:]]|$)/) {
        report(line, "curl needs -f so an HTTP error fails (SH-12)")
      }
      if (rest !~ /(--max-time|(^|[[:space:]])-[a-zA-Z]*m[[:space:]]*[0-9])/) {
        report(line, "curl needs --max-time (SH-12)")
      }
    }
  }
  if (match(code, /(^|[;&|({}!`][[:space:]]*|[[:space:]]|\$\()eval([[:space:]]|$)/)) {
    report(line, "no eval; build the command in an array (SH-6)")
  }
  if (match(code, /(^|[;&|({}!`][[:space:]]*|[[:space:]]|\$\()(MSYS_NO_PATHCONV=1[[:space:]]+)?(wsl|wsl\.exe|ssh)([[:space:]]|$)/)) {
    if (substr(code, 1, RSTART) !~ /timeout[[:space:]]+(-[^[:space:]]+[[:space:]]+)*[0-9]/) {
      report(line, "run wsl and ssh under timeout <seconds> (SH-14)")
    }
  }
  if (match(code, /(^|[;&|({}!`][[:space:]]*|[[:space:]]|\$\()(timeout[[:space:]]+[0-9]+[[:space:]]+)?(wsl|wsl\.exe)([[:space:]]|$)/)) {
    if (expand ~ /(^|[^A-Za-z0-9_])\/mnt\// && substr(code, 1, RSTART + RLENGTH) !~ /MSYS_NO_PATHCONV=1/) {
      report(line, "put MSYS_NO_PATHCONV=1 in front of a wsl call that passes a /mnt/ path (SH-9)")
    }
  }
  if (code ~ /(^|[;&|{}()[:space:]])exit[[:space:]]+([3-9]|[1-9][0-9]+)([^0-9]|$)/) {
    report(line, "exit with 0, 1 or 2 (SH-13)")
  }
  if (expand ~ /\$\$/ || expand ~ /(^|[^A-Za-z0-9_.~-])\/tmp\//) {
    report(line, "temporary files go in mktemp -d, not /tmp/ or $$ (SH-5)")
  }
  check_names(code, line)
  name = ""
  if (match(code, /^[[:space:]]*(function[[:space:]]+)?[A-Za-z_][A-Za-z0-9_:.-]*[[:space:]]*\(\)/)) {
    name = substr(code, RSTART, RLENGTH)
    sub(/[[:space:]]*\(\)$/, "", name)
  } else if (match(code, /^[[:space:]]*function[[:space:]]+[A-Za-z_][A-Za-z0-9_:.-]*/)) {
    name = substr(code, RSTART, RLENGTH)
  }
  if (name != "") {
    sub(/^[[:space:]]*(function[[:space:]]+)?/, "", name)
    if (name !~ /^[a-z_][a-z0-9_]*$/) {
      report(line, "function names are lower_snake_case (SH-11)")
    }
    if (name == "main") {
      report(line, "the main body runs at the top level, with no main function (SH-18)")
    }
    if (cur_fn == "") {
      cur_fn = name
      split("", locals)
      match(code, /^[[:space:]]*/)
      func_indent = substr(code, 1, RLENGTH)
      functions[name] = line
      rest = substr(code, index(code, "{") + 1)
      if (index(code, "{") > 0 && rest ~ /}[[:space:]]*$/) {
        sub(/}[[:space:]]*$/, "", rest)
        count = split_steps(rest, rest, starts, ends)
        for (i = 1; i <= count; i++) {
          step = substr(rest, starts[i], ends[i] - starts[i] + 1)
          note_conditions(step, line)
          note_step(step, line)
        }
        cur_fn = ""
      }
      return
    }
  }
  if (cur_fn != "" && code ~ ("^" func_indent "}")) {
    cur_fn = ""
    return
  }
  count = split_steps(code, text, starts, ends)
  for (i = 1; i <= count; i++) {
    step = substr(code, starts[i], ends[i] - starts[i] + 1)
    if (step ~ /^[[:space:]]*done([[:space:]]|$)/ && loops > 0) {
      loops--
    }
    if (loops > 0 && step ~ /(^|[;&|({}!`[:space:]]|\$\()(cut|sed|basename|dirname)([[:space:]]|$)/) {
      report(line, "use parameter expansion inside a loop, not cut, sed, basename or dirname (SH-15)")
    }
    if (step ~ /(^|\|)[[:space:]]*(for|while|until|select)[[:space:]]/) {
      loops++
    }
    if (step ~ /^[[:space:]]*(then|do|else|\{)?[[:space:]]*exit[[:space:]]+[12]([^0-9]|$)/ && printed) {
      report(line, "send the error to stderr through die (SH-4)")
    }
    printed = step ~ /(^|[{&|)]|then|do|else)[[:space:]]*(echo|printf)[[:space:]]/ && step !~ />/
    note_conditions(step, line)
    if (cur_fn != "") {
      note_step(step, line)
    }
  }
}

function heredoc_start(code, text, at, rest, word) {
  at = index(code, "<<")
  while (at > 0) {
    rest = substr(text, at + 2)
    if (substr(rest, 1, 1) != "<" && substr(code, 1, at - 1) !~ /\(\([^)]*$/) {
      heredoc_tabs = substr(rest, 1, 1) == "-"
      sub(/^-?[[:space:]]*/, "", rest)
      word = rest
      sub(/[[:space:];&|<>)].*$/, "", word)
      gsub(/["'\\]/, "", word)
      if (word ~ /^[A-Za-z_][A-Za-z0-9_]*$/) {
        return word
      }
    }
    code = substr(code, at + 3)
    text = substr(text, at + 3)
    at = index(code, "<<")
  }
  return ""
}

function finish_file(name, flat, i) {
  if (joined != "") {
    check(joined, start)
  }
  for (name in functions) {
    if (!(name in calls)) {
      continue
    }
    for (i = 1; i < body_count[name]; i++) {
      flat = name SUBSEP i
      if (!body_safe[flat]) {
        report(body_line[flat], name " runs from if, !, && or || (line " calls[name] "); end this step with || return (SH-3)")
      }
    }
  }
  split("", functions)
  split("", calls)
  split("", body_count)
  split("", body_line)
  split("", body_safe)
  joined = ""
  cur_fn = ""
  heredoc = ""
  loops = 0
  printed = 0
}

FNR == 1 {
  if (cur_file != "") {
    finish_file()
  }
  cur_file = FILENAME
  path = FILENAME
  sub(/^\.\//, "", path)
  listed[path] = 1
  shell = ""
  if ($0 ~ /^#!.*bash/) {
    shell = "bash"
  } else if ($0 ~ /^#!.*sh/) {
    shell = "sh"
  }
  if ($0 != "#!/usr/bin/env bash" && $0 != "#!/bin/sh") {
    report(FNR, "first line is #!/usr/bin/env bash, or #!/bin/sh where there is no bash (SH-1)")
  }
  next
}

FNR == 2 && shell == "bash" && $0 != "set -euo pipefail" {
  report(FNR, "second line is set -euo pipefail (SH-2)")
}

FNR == 3 && shell == "bash" && $0 != "shopt -s inherit_errexit" {
  report(FNR, "third line is shopt -s inherit_errexit (SH-2)")
}

FNR == 2 && shell == "sh" && $0 != "set -eu" {
  report(FNR, "second line is set -eu (SH-2)")
}

FNR == 101 {
  report(FNR, "over 100 lines; port it to Python or PowerShell (SH-10)")
}

heredoc != "" {
  line_text = $0
  if (heredoc_tabs) {
    sub(/^\t+/, "", line_text)
  }
  if (line_text == heredoc) {
    heredoc = ""
  }
  next
}

length($0) > 120 {
  report(FNR, "lines are at most 120 characters (SH-18)")
}

{
  code = scan($0, "code")
  if (match(code, /(^|[[:space:]])#/)) {
    if (substr($0, RSTART) ~ /^[[:space:]]*#[[:space:]]*shellcheck[[:space:]]/) {
      report(FNR, "no shellcheck directives; fix the code (GEN-4)")
    } else {
      report(FNR, "remove the comment; names carry the meaning (GEN-29)")
    }
    $0 = substr($0, 1, RSTART - 1)
  }
  if (joined == "") {
    start = FNR
  }
  text = $0
  continued = sub(/\\$/, "", text)
  joined = joined text
  if (!continued) {
    word = heredoc_start(scan(joined, "code"), joined)
    check(joined, start)
    joined = ""
    if (word != "") {
      heredoc = word
    }
  }
}

function sourced_names(text, rest, word) {
  rest = text
  while (match(rest, /(^|[;&|{(][[:space:]]*|[[:space:]])(source|\.)[[:space:]]+[^[:space:];&|]+/)) {
    word = substr(rest, RSTART, RLENGTH)
    rest = substr(rest, RSTART + RLENGTH)
    sub(/^.*(source|\.)[[:space:]]+/, "", word)
    gsub(/["']/, "", word)
    sub(/^.*\//, "", word)
    sourced[word] = 1
  }
}

{
  sourced_names($0)
}

END {
  if (cur_file != "") {
    finish_file()
  }
  count = split(eols, rows, "\n")
  for (i = 1; i <= count; i++) {
    if (split(rows[i], cols, "\t") < 2) {
      continue
    }
    split(cols[1], flags, /[[:space:]]+/)
    if ((flags[1] != "i/lf" && flags[1] != "i/none") || (flags[2] != "w/lf" && flags[2] != "w/none")) {
      report_path(cols[2], "check it in and out as LF through .gitattributes eol=lf (SH-8)")
    }
  }
  count = split(modes, rows, "\n")
  for (i = 1; i <= count; i++) {
    if (split(rows[i], cols, "\t") < 2) {
      continue
    }
    split(cols[1], flags, /[[:space:]]+/)
    path = cols[2]
    base = path
    sub(/^.*\//, "", base)
    if (base ~ /\.(sh|bash)\.tpl$/) {
      continue
    }
    if (base ~ /^.+\./ && base !~ /\.sh$/) {
      report_path(path, "executables have no extension or .sh; libraries end in .sh (SH-19)")
    }
    if (base in sourced) {
      if (flags[1] != "100644") {
        report_path(path, "a sourced library isn't executable: git update-index --chmod=-x (SH-19)")
      }
    } else if (flags[1] != "100755") {
      report_path(path, "set the exec bit in git: git update-index --chmod=+x (SH-20)")
    } else if (path ~ /^\.githooks\// && base ~ /\./) {
      report_path(path, "hooks in .githooks/ have no extension (SH-19)")
    }
  }
  exit bad
}
