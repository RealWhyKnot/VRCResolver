FNR == NR {
  sub(/\r$/, "")
  entry = $0
  gsub(/^[[:space:]]+|[[:space:]]+$/, "", entry)
  if (entry == "") {
    next
  }
  if (entry ~ /^\/.*[^\/]$/) {
    files[substr(entry, 2)] = 1
  } else if (entry ~ /^\//) {
    sub(/^\/+/, "", entry)
    sub(/\/+$/, "", entry)
    prefixes[++prefix_count] = entry "/"
  } else {
    sub(/^\/+/, "", entry)
    sub(/\/+$/, "", entry)
    names[entry] = 1
  }
  next
}

function is_vendored(path, parts, count, position) {
  if (path in files) {
    return 1
  }
  for (position = 1; position <= prefix_count; position++) {
    if (substr(path, 1, length(prefixes[position])) == prefixes[position]) {
      return 1
    }
  }
  count = split(path, parts, "/")
  for (position = 1; position < count; position++) {
    if (parts[position] in names) {
      return 1
    }
  }
  return 0
}

{
  path = $0
  if (path == "" || is_vendored(path)) {
    next
  }
  base = path
  sub(/^.*\//, "", base)
  if (base ~ /\.(sh|bash)(\.tpl)?$/) {
    print path
    next
  }
  if (base !~ /^\.?[^.]*$/) {
    next
  }
  first = ""
  if ((getline first < path) > 0) {
    if (first ~ /^#![ \t]*[^ \t]*\/(env[ \t]+(-[^ \t]+[ \t]+)*)?(ba|da|k|z)?sh([^A-Za-z0-9_]|$)/) {
      print path
    }
  }
  close(path)
}
