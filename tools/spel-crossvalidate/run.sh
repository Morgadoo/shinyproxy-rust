#!/usr/bin/env bash
# Cross-validates the `spel` crate against the real Spring Expression Language.
#
# It extracts the expressions from the compatibility corpus
# (crates/spel/tests/corpus.rs), evaluates them with Spring (downloading
# spring-expression from Maven Central into a cache directory) and reports every
# expression where the two implementations disagree.
#
# Usage: tools/spel-crossvalidate/run.sh
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="${SPEL_CROSSVALIDATE_DIR:-/tmp/spel-crossvalidate}"
SPRING_VERSION="${SPRING_VERSION:-6.2.14}"
mkdir -p "$WORK"
cd "$WORK"

for artifact in spring-expression spring-core spring-jcl; do
    jar="${artifact}-${SPRING_VERSION}.jar"
    if [[ ! -f "$jar" ]]; then
        echo "downloading $jar"
        curl -sfL -O "https://repo1.maven.org/maven2/org/springframework/${artifact}/${SPRING_VERSION}/${jar}"
    fi
done
CP="spring-expression-${SPRING_VERSION}.jar:spring-core-${SPRING_VERSION}.jar:spring-jcl-${SPRING_VERSION}.jar"

cp "$REPO_ROOT/tools/spel-crossvalidate/SpelRef.java" .
javac -cp "$CP" SpelRef.java

python3 - "$REPO_ROOT/crates/spel/tests/corpus.rs" <<'PYTHON'
import json, pathlib, re, sys

text = pathlib.Path(sys.argv[1]).read_text()
start = text.index('const CORPUS: &[(&str, &str)] = &[')
end = text.index('];', start)
pairs = re.findall(r'\(\s*("(?:[^"\\]|\\.)*")\s*,\s*("(?:[^"\\]|\\.)*")\s*\)', text[start:end])
expressions, expected = [], []
for raw_expression, raw_value in pairs:
    expression = json.loads(raw_expression.replace("\\'", "'"))
    value = json.loads(raw_value.replace("\\'", "'"))
    if expression == "":
        continue  # the empty template has no Java equivalent
    expressions.append(expression)
    expected.append(f"{expression}\t{value}")
pathlib.Path("corpus.txt").write_text("\n".join(expressions) + "\n")
pathlib.Path("expected.tsv").write_text("\n".join(expected) + "\n")
print(f"extracted {len(expressions)} expressions from the corpus")
PYTHON

# the corpus expects these values in the environment
SP_DATA_DIR=/data HOME=/home/shinyproxy java -cp "$CP:." SpelRef corpus.txt > java-results.tsv

python3 - <<'PYTHON'
import pathlib

def read(path):
    result = {}
    for line in pathlib.Path(path).read_text().splitlines():
        if not line:
            continue
        parts = line.split("\t")
        result[parts[0]] = parts[1] if len(parts) > 1 else ""
    return result

expected = read("expected.tsv")
java = read("java-results.tsv")

# Documented supersets: this implementation accepts more than Spring does. Configurations that work
# with Java therefore keep working; see docs/COMPATIBILITY.md.
supersets = {
    "#{oidcUser?.attributes?.dept}",
    "#{'a,b,c'.split(',').size()}",
}

mismatches = []
for expression, value in expected.items():
    actual = java.get(expression, "<<missing>>")
    if actual != value and expression not in supersets:
        mismatches.append((expression, value, actual))

print(f"compared {len(expected)} expressions: {len(mismatches)} mismatch(es), "
      f"{len(supersets)} documented superset(s)")
for expression, rust, java_value in mismatches:
    print(f"\nexpression: {expression}\n  rust: {rust!r}\n  java: {java_value!r}")
raise SystemExit(1 if mismatches else 0)
PYTHON
