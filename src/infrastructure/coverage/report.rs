// How the audit prints itself: one funnel row per root, then the named losses of each root.
//
// The report is printed rather than written to a file. The numbers go into
// `docs/coverage-audit.md` by hand together with the date the run was made, because the
// tree they describe keeps changing and a number without its date is a claim about nothing.
use super::funnel::RootFunnel;

/// Digits grouped by a space, the way every measured number in `docs/` is written.
///
/// The terminal's own `format_number` groups by a comma and lives in `presentation`, which
/// a measurement in `infrastructure` may not reach into. One small function that says which
/// of the two shapes this report uses is the smaller of the two prices.
pub(super) fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(' ');
        }
        grouped.push(digit);
    }
    grouped
}

pub(super) fn print_funnel_header() {
    println!(
        "| контекст | корень | на диске | допущено | в графе | ревизий с текстом | чанков | \
         проходов |"
    );
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |");
}

pub(super) fn print_funnel_row(funnel: &RootFunnel) {
    let root = funnel.root().rsplit('/').next().unwrap_or(funnel.root());
    println!(
        "| {} | `{}` | {} | {} | {} | {} | {} | {} |",
        funnel.context(),
        root,
        grouped(funnel.entries_on_disk()),
        grouped(funnel.sieve().reaches_the_graph()),
        grouped(funnel.graph().files()),
        grouped(funnel.graph().files_with_current_version()),
        grouped(funnel.graph().chunks_of_current_versions()),
        grouped(funnel.graph().passes()),
    );
}

/// Every named loss of one root, in the order the funnel loses them.
pub(super) fn print_root_detail(funnel: &RootFunnel, failures: &[String], failure_count: usize) {
    let sieve = funnel.sieve();
    let content = funnel.content();
    println!();
    println!("### {} · {}", funnel.context(), funnel.root());
    println!(
        "на диске (не каталогов): {}",
        grouped(funnel.entries_on_disk())
    );

    println!("политика отсеяла:");
    for (pattern, count) in sieve.excluded_by_rule() {
        println!("  {:>10}  правило {pattern}", grouped(count));
    }
    print_if_any(
        "  правило не покрывает путь (отказ по умолчанию)",
        sieve.excluded_by_default(),
    );
    print_if_any(
        &format!(
            "  под каталогом с маркером ({} каталогов)",
            grouped(sieve.generated_directories())
        ),
        sieve.under_generated_directory(),
    );
    print_if_any("  имя не в UTF-8", sieve.unrepresentable_names());

    println!(
        "допущено политикой: {} (из них только метаданными по правилу: {})",
        grouped(sieve.reaches_the_graph()),
        grouped(sieve.metadata_only())
    );

    println!("содержимое допущенных:");
    print_if_any("  прочитано в чанки", content.revisions_with_text());
    print_if_any("  прочитано, текста нет", content.held_no_text());
    print_if_any("  двоичный", content.binary());
    print_if_any("  не UTF-8", content.not_utf8());
    print_if_any("  крупнее предела", content.over_the_content_limit());
    print_if_any(
        "  не обычный файл (ссылка, спецфайл)",
        content.not_a_regular_file(),
    );
    print_if_any(
        "  изменился во время чтения",
        content.changed_while_reading(),
    );
    print_if_any("  не удалось описать или открыть", content.unreadable());
    println!("  чанков всего: {}", grouped(content.chunks()));

    println!(
        "граф: файлов {}, с текущей версией {}, чанков текущих версий {}",
        grouped(funnel.graph().files()),
        grouped(funnel.graph().files_with_current_version()),
        grouped(funnel.graph().chunks_of_current_versions()),
    );
    println!(
        "  мест: present {}, missing {}",
        grouped(funnel.graph().locations_present()),
        grouped(funnel.graph().locations_missing()),
    );
    println!(
        "  проходов reconcile по этому корню: {}",
        grouped(funnel.graph().passes())
    );
    println!(
        "РАЗРЫВ (допущено − в графе): {}",
        funnel.files_the_graph_is_missing()
    );

    if failure_count > 0 {
        println!("отказов обхода: {}", grouped(failure_count));
        for failure in failures {
            println!("  {failure}");
        }
    }
}

fn print_if_any(label: &str, count: usize) {
    if count > 0 {
        println!("{label}: {}", grouped(count));
    }
}

#[cfg(test)]
mod tests {
    use super::grouped;

    #[test]
    fn numbers_are_grouped_by_a_space_from_the_right() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1 000");
        assert_eq!(grouped(367_341), "367 341");
        assert_eq!(grouped(1_234_567), "1 234 567");
    }
}
