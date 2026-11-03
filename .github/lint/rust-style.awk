BEGIN {
  while ((status = (getline entry < ".github/lint/vendored-paths.txt")) > 0) {
    sub(/\r$/, "", entry)
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", entry)
    if (entry == "") {
      continue
    }
    sub(/\/*$/, "/", entry)
    if (substr(entry, 1, 1) == "/") {
      roots[++root_count] = substr(entry, 2)
    } else {
      names[++name_count] = "/" entry
    }
  }
  if (status < 0) {
    print ".github/lint/vendored-paths.txt is missing. Run Install-Lint again." > "/dev/stderr"
    broken = 1
    exit
  }
}

function report(file, line, msg) {
  printf "%s:%d: %s\n", file, line, msg
  bad = 1
}

function vendored(path, i) {
  for (i = 1; i <= root_count; i++) {
    if (index(path, roots[i]) == 1) {
      return 1
    }
  }
  for (i = 1; i <= name_count; i++) {
    if (index("/" path, names[i]) > 0) {
      return 1
    }
  }
  return 0
}

function count(text, re, copy) {
  copy = text
  return gsub(re, "", copy)
}

function check_attr(text, file, line) {
  if (text ~ /#!?\[[[:space:]]*rustfmt::skip/ || text ~ /#!?\[[[:space:]]*cfg_attr[[:space:]]*\(.*rustfmt::skip/) {
    report(file, line, "remove rustfmt::skip and let rustfmt format it (RS-3)")
  }
}

function check_line(text, line) {
  if (text ~ /(^|[^a-z_])pub[[:space:]]*(\([[:space:]]*(crate|in[[:space:]][^)]*)[[:space:]]*\))?[[:space:]]+(const[[:space:]]+)?(async[[:space:]]+)?unsafe[[:space:]]+(extern[[:space:]]*("[^"]*"[[:space:]]*)?)?fn[[:space:]]/) {
    report(FILENAME, line, "an unsafe fn is private or pub(super); expose a safe fn instead (RS-2)")
  }
  if (text ~ /map_err\([[:space:]]*\|[[:space:]]*[a-z_][a-z0-9_]*[[:space:]]*\|[[:space:]]*[a-z_][a-z0-9_]*\.to_string\(\)[[:space:]]*\)/ || text ~ /map_err\([[:space:]]*\|[^|]*\|[[:space:]]*format!\([[:space:]]*"\{[a-z_0-9]*\}"[[:space:]]*(,[[:space:]]*[a-z_][a-z0-9_]*[[:space:]]*)?\)[[:space:]]*\)/ || text ~ /map_err\([[:space:]]*(std::string::)?ToString::to_string[[:space:]]*\)/) {
    report(FILENAME, line, "add what was being done to the error text (rust.md §3)")
  }
}

FNR == 1 {
  if (attr != "") {
    check_attr(attr, attr_file, start)
  }
  attr = ""
  skip = vendored(FILENAME)
}

skip {
  next
}

{
  code = $0
  gsub(/"([^"\\]|\\.)*"/, "\"\"", code)
  if (attr == "" && match(code, /#!?\[/)) {
    attr = substr(code, RSTART)
    attr_file = FILENAME
    start = FNR
  } else if (attr != "") {
    attr = attr " " code
  }
  if (attr != "" && (count(attr, "\\[") <= count(attr, "\\]") || FNR - start > 30)) {
    check_attr(attr, attr_file, start)
    attr = ""
  }
  check_line($0, FNR)
}

END {
  if (broken) {
    exit 2
  }
  if (attr != "") {
    check_attr(attr, attr_file, start)
  }
  exit bad
}
