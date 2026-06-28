#!/bin/bash

echo "File Path,Lines,if_let,unwrap_or,unwrap,else_if,string_replace_tests,Tech_Debt_Score" > tech_debt_scores_all.csv

# Find all relevant code files (.rs, .py, .ts, etc) excluding tests where appropriate, but including the python scripts dir
for file in $(find src scripts -type f \( -name "*.rs" -o -name "*.py" -o -name "*.sh" \)); do
    lines=$(wc -l < "$file")

    # Ignore tiny files (< 20 lines)
    if [ "$lines" -lt 20 ]; then
        continue
    fi

    # Rust patterns
    if_let_count=$(grep -c "if let" "$file")
    unwrap_or_count=$(grep -c "unwrap_or" "$file")
    unwrap_count=$(grep -c "\.unwrap()" "$file")
    else_if_count=$(grep -c "else if" "$file")

    # Python/Scripting patterns (e.g., test string mutations)
    replace_count=$(grep -c "replace(" "$file")
    replace_once_count=$(grep -c "replace_once(" "$file")

    # Calculate a rough "Tech Debt Score"
    raw_score=$(( (if_let_count * 1) + (else_if_count * 2) + (unwrap_or_count * 3) + (unwrap_count * 4) + (replace_count * 3) + (replace_once_count * 4) ))

    # Normalizing score per 1000 lines
    if [ "$lines" -gt 0 ]; then
        normalized_score=$(( raw_score * 1000 / lines ))
    else
        normalized_score=0
    fi

    echo "$file,$lines,$if_let_count,$unwrap_or_count,$unwrap_count,$else_if_count,$((replace_count + replace_once_count)),$normalized_score" >> tech_debt_scores_all.csv
done

echo "--- Highest Tech Debt (Normalized Density per 1000 lines) ---"
printf "%-60s %-8s %-10s %-8s %-8s %-8s %-12s %-10s\n" "File Path" "Lines" "if let" "unwrap_or" "unwrap" "else if" "String Replace" "Density_Score"
sort -t, -k8 -nr tech_debt_scores_all.csv | head -n 15 | awk -F, '{printf "%-60s %-8s %-10s %-8s %-8s %-8s %-12s %-10s\n", $1, $2, $3, $4, $5, $6, $7, $8}'
