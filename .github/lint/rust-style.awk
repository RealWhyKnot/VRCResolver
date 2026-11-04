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
    is_broken = 1
    exit
  }
}

function report(file, line, message) {
  printf "%s:%d: %s\n", file, line, message
  has_findings = 1
}

function is_vendored(path, position) {
  for (position = 1; position <= root_count; position++) {
    if (index(path, roots[position]) == 1) {
      return 1
    }
  }
  for (position = 1; position <= name_count; position++) {
    if (index("/" path, names[position]) > 0) {
      return 1
    }
  }
  return 0
}

function count(text, pattern, copy) {
  copy = text
  return gsub(pattern, "", copy)
}

function check_attribute(text, file, line) {
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
  if (attribute != "") {
    check_attribute(attribute, attribute_file, start)
  }
  attribute = ""
  is_vendored_file = is_vendored(FILENAME)
}

is_vendored_file {
  next
}

{
  code = $0
  gsub(/"([^"\\]|\\.)*"/, "\"\"", code)
  if (attribute == "" && match(code, /#!?\[/)) {
    attribute = substr(code, RSTART)
    attribute_file = FILENAME
    start = FNR
  } else if (attribute != "") {
    attribute = attribute " " code
  }
  if (attribute != "" && (count(attribute, "\\[") <= count(attribute, "\\]") || FNR - start > 30)) {
    check_attribute(attribute, attribute_file, start)
    attribute = ""
  }
  check_line($0, FNR)
}

END {
  if (is_broken) {
    exit 2
  }
  if (attribute != "") {
    check_attribute(attribute, attribute_file, start)
  }
  exit has_findings
}
