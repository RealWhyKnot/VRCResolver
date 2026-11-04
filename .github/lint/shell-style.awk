function report(line, message) {
  printf "%s:%d: %s\n", current_file, line, message
  has_findings = 1
}

function report_path(path, message) {
  printf "%s: %s\n", path, message
  has_findings = 1
}

function pop(stack) {
  return substr(stack, 1, length(stack) - 1)
}

function scan(text, mode, output, position, text_length, character, stack, top) {
  output = ""
  stack = ""
  text_length = length(text)
  for (position = 1; position <= text_length; position++) {
    character = substr(text, position, 1)
    top = substr(stack, length(stack), 1)
    if (top == "'") {
      if (character == "'") {
        stack = pop(stack)
        output = output character
      } else if (mode == "code") {
        output = output "_"
      }
      continue
    }
    if (character == "\\") {
      position++
      output = output "__"
      continue
    }
    if (top == "\"") {
      if (character == "\"") {
        stack = pop(stack)
        output = output character
      } else if (character == "$" && substr(text, position + 1, 1) == "(") {
        stack = stack "("
        output = output "$("
        position++
      } else if (mode == "code") {
        output = output "_"
      } else {
        output = output character
      }
      continue
    }
    if (top == "(" && character == ")") {
      stack = pop(stack)
    } else if (top == "(" && character == "(") {
      stack = stack "("
    } else if (character == "'" || character == "\"") {
      stack = stack character
    }
    output = output character
  }
  return output
}

function drop_substitution(text, output, position, text_length, character, depth) {
  output = ""
  depth = 0
  text_length = length(text)
  for (position = 1; position <= text_length; position++) {
    character = substr(text, position, 1)
    if (character == "$" && substr(text, position + 1, 1) == "(") {
      depth++
      position++
    } else if (depth > 0 && character == "(") {
      depth++
    } else if (depth > 0 && character == ")") {
      depth--
    } else if (depth == 0) {
      output = output character
    }
  }
  return output
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

function split_steps(code, text, starts, ends, step_count, position, character, from, depth) {
  step_count = 0
  from = 1
  depth = 0
  for (position = 1; position <= length(code); position++) {
    character = substr(code, position, 1)
    if (character == "$" && substr(code, position + 1, 1) == "(") {
      depth++
      position++
    } else if (depth > 0 && character == ")") {
      depth--
    } else if (depth == 0 && character == ";") {
      step_count++
      starts[step_count] = from
      ends[step_count] = position - 1
      from = position + 1
    }
  }
  step_count++
  starts[step_count] = from
  ends[step_count] = length(code)
  return step_count
}

function note_conditions(step, line, parts, count, position, word) {
  step = drop_substitution(step)
  if (match(step, /^[[:space:]]*(if|elif|while|until|!)[[:space:]]+/)) {
    word = first_word(substr(step, RLENGTH + 1))
    if (word != "") {
      calls[word] = line
    }
  }
  gsub(/&&|\|\|/, "\001", step)
  count = split(step, parts, "\001")
  for (position = 1; position < count; position++) {
    word = parts[position]
    sub(/^[[:space:]]*(if|elif|while|until)[[:space:]]+/, "", word)
    word = first_word(word)
    if (word != "") {
      calls[word] = line
    }
  }
}

function note_step(step, line, plain, name, rest, words, count, position, flat) {
  plain = trim(drop_substitution(step))
  sub(/^[^[:space:]()]*\)[[:space:]]*/, "", plain)
  if (plain == "" || plain ~ /^(then|do|else|fi|done|esac|\{|\}|;|\)|\(|in)$/) {
    return
  }
  if (plain ~ /^(if|elif|while|until|for|case|select|function)[[:space:]]/ || plain ~ /^[^[:space:]]*\)$/) {
    return
  }
  if (plain ~ /^(local|declare|typeset|readonly)([[:space:]]|$)/) {
    count = split(plain, words, /[[:space:]]+/)
    for (position = 2; position <= count; position++) {
      name = words[position]
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
  body_count[current_function]++
  flat = current_function SUBSEP body_count[current_function]
  body_line[flat] = line
  body_safe[flat] = plain ~ /\|\|[[:space:]]*(return|exit|die|\{)([[:space:]]|$)/
}

function check_names(code, line, parts, count, position, words, total, word_position, word, is_declaring, name) {
  gsub(/\$\(|[;&|(){}]/, "\001", code)
  count = split(code, parts, "\001")
  for (position = 1; position <= count; position++) {
    total = split(trim(parts[position]), words, /[[:space:]]+/)
    is_declaring = 0
    for (word_position = 1; word_position <= total; word_position++) {
      word = words[word_position]
      if (!is_declaring && word ~ /^(then|do|else|elif|if|while|until|!|time)$/) {
        continue
      }
      if (!is_declaring && word ~ /^(local|declare|typeset|readonly|export)$/) {
        is_declaring = 1
        continue
      }
      if (is_declaring && word ~ /^-/) {
        continue
      }
      if (!match(word, /^[A-Za-z_][A-Za-z0-9_]*([+]?=|$)/)) {
        break
      }
      name = word
      sub(/[+]?=.*$/, "", name)
      if (!is_declaring && name == word) {
        break
      }
      if (name !~ /^[a-z_][a-z0-9_]*$/ && name !~ /^[A-Z_][A-Z0-9_]*$/) {
        report(line, "variable " name " is lower_snake_case, or UPPER_CASE when exported or constant (SH-11)")
      }
    }
  }
}

function check(text, line, code, expand, rest, name, steps, starts, ends, count, position, step) {
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
    if (current_function == "") {
      current_function = name
      split("", locals)
      match(code, /^[[:space:]]*/)
      func_indent = substr(code, 1, RLENGTH)
      functions[name] = line
      rest = substr(code, index(code, "{") + 1)
      if (index(code, "{") > 0 && rest ~ /}[[:space:]]*$/) {
        sub(/}[[:space:]]*$/, "", rest)
        count = split_steps(rest, rest, starts, ends)
        for (position = 1; position <= count; position++) {
          step = substr(rest, starts[position], ends[position] - starts[position] + 1)
          note_conditions(step, line)
          note_step(step, line)
        }
        current_function = ""
      }
      return
    }
  }
  if (current_function != "" && code ~ ("^" func_indent "}")) {
    current_function = ""
    return
  }
  count = split_steps(code, text, starts, ends)
  for (position = 1; position <= count; position++) {
    step = substr(code, starts[position], ends[position] - starts[position] + 1)
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
    if (current_function != "") {
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

function finish_file(name, flat, position) {
  if (joined != "") {
    check(joined, start)
  }
  for (name in functions) {
    if (!(name in calls)) {
      continue
    }
    for (position = 1; position < body_count[name]; position++) {
      flat = name SUBSEP position
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
  current_function = ""
  heredoc = ""
  loops = 0
  printed = 0
}

FNR == 1 {
  if (current_file != "") {
    finish_file()
  }
  current_file = FILENAME
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
  is_continued = sub(/\\$/, "", text)
  joined = joined text
  if (!is_continued) {
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
  if (current_file != "") {
    finish_file()
  }
  count = split(eols, rows, "\n")
  for (position = 1; position <= count; position++) {
    if (split(rows[position], columns, "\t") < 2) {
      continue
    }
    split(columns[1], flags, /[[:space:]]+/)
    if ((flags[1] != "i/lf" && flags[1] != "i/none") || (flags[2] != "w/lf" && flags[2] != "w/none")) {
      report_path(columns[2], "check it in and out as LF through .gitattributes eol=lf (SH-8)")
    }
  }
  count = split(modes, rows, "\n")
  for (position = 1; position <= count; position++) {
    if (split(rows[position], columns, "\t") < 2) {
      continue
    }
    split(columns[1], flags, /[[:space:]]+/)
    path = columns[2]
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
  exit has_findings
}
