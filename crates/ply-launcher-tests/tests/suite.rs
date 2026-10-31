#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

#[test]
fn the_count_allocations_flag_is_read_from_the_line_and_taken_out() {
    for line in [
        vec!["--count-allocations=out.json", "run", "p.ply"],
        vec!["run", "--count-allocations=out.json", "p.ply"],
        vec!["run", "p.ply", "--count-allocations=out.json"],
    ] {
        let mut argv: Vec<String> = line.iter().map(|s| s.to_string()).collect();
        let asked = ply_launcher::count::flag(&mut argv).unwrap().unwrap();
        assert_eq!(asked.path, std::path::PathBuf::from("out.json"));
        assert!(!asked.sites);
        assert!(!asked.exact);
        assert_eq!(asked.every(), 0, "no sites, so nothing is walked");
        assert_eq!(
            argv,
            vec!["run".to_string(), "p.ply".to_string()],
            "the flag was left in the line"
        );
    }
}

#[test]
fn the_count_allocations_flag_may_name_its_path_with_a_space() {
    let mut argv: Vec<String> = ["run", "--count-allocations", "out.json", "p.ply"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let asked = ply_launcher::count::flag(&mut argv).unwrap().unwrap();
    assert_eq!(asked.path, std::path::PathBuf::from("out.json"));
    assert!(!asked.sites);
    assert_eq!(argv, vec!["run".to_string(), "p.ply".to_string()]);
}

#[test]
fn the_count_allocations_flag_requires_a_path() {
    let mut argv: Vec<String> = ["run", "--count-allocations"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(ply_launcher::count::flag(&mut argv).is_err());
}

#[test]
fn a_line_without_the_flag_keeps_its_words() {
    let mut argv: Vec<String> = ["run", "p.ply"].iter().map(|s| s.to_string()).collect();
    assert!(ply_launcher::count::flag(&mut argv).unwrap().is_none());
    assert_eq!(argv, vec!["run".to_string(), "p.ply".to_string()]);
}

#[test]
fn a_window_counts_what_it_ran_and_only_attributes_when_asked() {
    let (result, counted, sites) = ply_launcher::count::window(
        || {
            let v: Vec<u64> = (0..64u64).map(|n| n * 2).collect();
            v.iter().sum::<u64>()
        },
        false,
    );
    assert_eq!(result, (0..64u64).map(|n| n * 2).sum::<u64>());
    assert!(counted.allocations > 0, "a vector allocates");
    assert!(counted.bytes >= 64 * 8);
    assert!(
        sites.is_empty(),
        "no sites were asked for, so none were recorded"
    );
    let (_, counted, sites) = ply_launcher::count::window(
        || {
            let v: Vec<u64> = (0..64u64).map(|n| n * 2).collect();
            v.len()
        },
        true,
    );
    assert!(counted.allocations > 0);
    assert!(!sites.is_empty(), "a window that asked for sites got none");
    let attributed: u64 = sites.values().map(|at| at.allocations).sum();
    assert_eq!(
        attributed, counted.allocations,
        "the rows are what the window counted, in full"
    );
    let bytes: u64 = sites.values().map(|at| at.bytes).sum();
    assert_eq!(
        bytes, counted.bytes,
        "the rows' bytes are the window's bytes"
    );
    let (result, counted, sites) = ply_launcher::count::window_sampled(
        || {
            let mut v: Vec<Vec<u64>> = Vec::new();
            for n in 0..4096u64 {
                v.push(vec![n; 4]);
            }
            v.len()
        },
        ply_launcher::count::SAMPLED,
    );
    assert_eq!(result, 4096);
    assert!(
        counted.allocations >= 4096,
        "4096 vectors allocate at least once each: {}",
        counted.allocations
    );
    assert!(!sites.is_empty(), "a long window sampled some sites");
    let walked: u64 = sites.values().map(|at| at.allocations).sum();
    assert!(
        walked < counted.allocations,
        "the sample is smaller than the window: {walked} of {}",
        counted.allocations
    );
    let scaled = walked * u64::from(ply_launcher::count::SAMPLED);
    let low = counted.allocations * 3 / 4;
    let high = counted.allocations * 5 / 4;
    assert!(
        (low..=high).contains(&scaled),
        "the scaled rows are near the total: {scaled} against {}",
        counted.allocations
    );
}

#[test]
fn the_sites_flags_choose_between_every_allocation_and_a_sample() {
    for (line, exact) in [
        (vec!["run", "--count-allocation-sites=out.json"], false),
        (vec!["run", "--count-allocation-sites-exact=out.json"], true),
        (
            vec![
                "run",
                "--count-allocation-sites=out.json",
                "--count-allocation-sites-exact=e.json",
            ],
            true,
        ),
        (
            vec![
                "run",
                "--count-allocation-sites-exact=e.json",
                "--count-allocation-sites=out.json",
            ],
            true,
        ),
    ] {
        let mut argv: Vec<String> = line.iter().map(|s| s.to_string()).collect();
        let asked = ply_launcher::count::flag(&mut argv).unwrap().unwrap();
        assert!(asked.sites, "sites were asked for: {line:?}");
        assert_eq!(asked.exact, exact, "{line:?}");
        assert_eq!(
            asked.every(),
            if exact {
                1
            } else {
                ply_launcher::count::SAMPLED
            },
            "{line:?}"
        );
    }
}

#[test]
fn the_sample_does_not_fall_on_one_period_of_the_window() {
    let every = ply_launcher::count::SAMPLED;
    let hits: Vec<u64> = (0..(1u64 << 20))
        .filter(|n| ply_launcher::count::sampled_nth(*n, every))
        .collect();
    let expected = (1usize << 20) / every as usize;
    assert!(
        hits.len() > expected / 2 && hits.len() < expected * 2,
        "{} sampled of an expected {expected}",
        hits.len()
    );
    assert!(
        hits.windows(2)
            .any(|pair| pair[1] - pair[0] != u64::from(every)),
        "every gap is {every}: the sample is periodic after all"
    );
    assert!(ply_launcher::count::sampled_nth(0, 1), "exact walks all");
    assert!(
        ply_launcher::count::sampled_nth(7, 0),
        "no sites walks none"
    );
}
