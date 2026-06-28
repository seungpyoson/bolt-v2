#!/bin/bash
echo "=== 1. Top Files using unwrap_or() ==="
grep -rn "unwrap_or(" src/ | grep -v tests | awk -F: '{print $1}' | sort | uniq -c | sort -nr | head -n 10

echo -e "\n=== 2. Top Files using unwrap_or_else() ==="
grep -rn "unwrap_or_else(" src/ | grep -v tests | awk -F: '{print $1}' | sort | uniq -c | sort -nr | head -n 10

echo -e "\n=== 3. Top Files using unwrap_or_default() ==="
grep -rn "unwrap_or_default(" src/ | grep -v tests | awk -F: '{print $1}' | sort | uniq -c | sort -nr | head -n 10

echo -e "\n=== 4. Top Files using unwrap() ==="
grep -rn "\.unwrap()" src/ | grep -v tests | awk -F: '{print $1}' | sort | uniq -c | sort -nr | head -n 10

echo -e "\n=== 5. Ignored Errors (if let Err) ==="
grep -rn "if let Err(" src/ | grep -v tests | awk -F: '{print $1}' | sort | uniq -c | sort -nr | head -n 10
