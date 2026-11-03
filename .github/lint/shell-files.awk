FNR == NR {
  sub(/\r$/, "")
  entry = $0
  gsub(/^[[:space:]]+|[[:space:]]+$/, "", entry)
  if (entry == "") {
    next
  }
  if (entry ~ /^\//) {
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

function vendored(path, parts, count, i) {
  for (i = 1; i <= prefix_count; i++) {
    if (substr(path, 1, length(prefixes[i])) == prefixes[i]) {
      return 1
    }
  }
  count = split(path, parts, "/")
  for (i = 1; i < count; i++) {
    if (parts[i] in names) {
      return 1
    }
  }
  return 0
}

{
  path = $0
  if (path == "" || vendored(path)) {
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
