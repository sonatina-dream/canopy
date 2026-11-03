//! Docking, removal, tabs, seams and persistence (SPEC §3, §4, §7, §8).

use std::collections::{BTreeMap, HashMap, HashSet};

use canopy_core::*;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn near_all(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| near(*x, *y))
}

fn tree(a: &Arrangement) -> String {
    a.describe(a.root())
}

fn weights(a: &Arrangement, id: NodeId) -> Vec<f64> {
    a.get(id).weights.clone()
}

fn all_weights(a: &Arrangement) -> Vec<Vec<f64>> {
    a.all_nodes(a.root()).into_iter().map(|n| weights(a, n)).collect()
}

fn resize(a: &mut Arrangement, index: usize, delta: f64, available: f64, limits: SeamLimits) {
    a.resize_seam(a.root(), index, index + 1, delta, available, limits);
}

mod docking {
    use super::*;

    #[test]
    fn a_fresh_world_is_just_the_desktop() {
        let a = Arrangement::new();
        assert!(a.get(a.root()).is_desktop());
        assert!(a.is_empty());
    }

    #[test]
    fn docking_at_the_left_screen_edge_splits_the_screen_with_the_desktop() {
        let mut a = Arrangement::new();
        a.dock_at_screen_edge(1, Edge::Left);
        assert_eq!(tree(&a), "H(1 D)");
        assert!(near_all(&weights(&a, a.root()), &[0.5, 0.5]));
    }

    #[test]
    fn docking_beside_a_window_in_the_same_axis_inserts_a_sibling_and_neighbours_make_room() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        assert_eq!(tree(&a), "H(1 2 D)");
        assert!(near_all(&weights(&a, a.root()), &[1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]));
    }

    #[test]
    fn docking_on_a_cross_axis_edge_nests_a_new_split_in_place_of_the_target() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Bottom, None);
        assert_eq!(tree(&a), "H(V(1 2) D)");
        let v = a.get(a.root()).children[0];
        assert!(near_all(&weights(&a, v), &[0.5, 0.5]));
        assert!(near_all(&weights(&a, a.root()), &[0.5, 0.5]), "the nested split keeps the target's share");
    }

    #[test]
    fn nesting_goes_as_deep_as_you_like() {
        let mut a = Arrangement::new();
        let mut t = a.dock_at_screen_edge(1, Edge::Left);
        for w in 2..=12 {
            t = a.dock(w, t, if w % 2 == 0 { Edge::Bottom } else { Edge::Right }, None);
        }
        assert_eq!(a.windows().len(), 12);
        assert_eq!(a.depth(a.tile_of(12).unwrap()), 12);
    }

    #[test]
    fn docking_at_a_screen_edge_goes_beside_everything() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Bottom, None);
        a.dock_at_screen_edge(3, Edge::Top);
        assert_eq!(tree(&a), "V(3 H(V(1 2) D))");
        a.dock_at_screen_edge(4, Edge::Bottom);
        assert_eq!(tree(&a), "V(3 H(V(1 2) D) 4)");
        assert!(near(weights(&a, a.root()).iter().sum(), 1.0));
    }

    #[test]
    fn docking_an_already_docked_window_moves_it_instead_of_duplicating_it() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        a.dock(1, a.tile_of(2).unwrap(), Edge::Bottom, None);
        let mut windows = a.windows();
        windows.sort_unstable();
        assert_eq!(windows, [1, 2]);
        assert_eq!(tree(&a), "H(V(2 1) D)");
    }

    #[test]
    fn there_is_no_limit_on_docked_windows() {
        let mut a = Arrangement::new();
        for w in 1..=40 {
            let rects = Solver::solve(&a, a.root(), Rect::new(0.0, 0.0, 4000.0, 3000.0), 0.0).node_rects;
            a.auto_dock(w, &rects);
        }
        assert_eq!(a.windows().len(), 40);
    }
}

mod removal {
    use super::*;

    #[test]
    fn closing_a_window_exactly_undoes_adding_it_when_space_is_shared() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let t2 = a.dock(2, t1, Edge::Right, None);
        resize(&mut a, 0, 100.0, 1000.0, SeamLimits::default());
        let before = weights(&a, a.root());
        a.dock(3, t2, Edge::Right, None);
        a.remove(3, true);
        assert_eq!(tree(&a), "H(1 2 D)");
        assert!(near_all(&weights(&a, a.root()), &before));
    }

    #[test]
    fn without_sharing_the_next_neighbour_takes_all_the_space() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        // H(1 2 D) with thirds; remove 1 -> 2 gets 2/3.
        a.remove(1, false);
        assert_eq!(tree(&a), "H(2 D)");
        assert!(near_all(&weights(&a, a.root()), &[2.0 / 3.0, 1.0 / 3.0]));
    }

    #[test]
    fn the_last_childs_space_goes_to_the_previous_neighbour_without_sharing() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let t2 = a.dock(2, t1, Edge::Bottom, None);
        a.dock(3, t2, Edge::Bottom, None);
        a.remove(3, false);
        let v = a.get(a.root()).children[0];
        assert!(near_all(&weights(&a, v), &[1.0 / 3.0, 2.0 / 3.0]));
    }

    #[test]
    fn a_split_left_with_one_child_collapses() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Bottom, None);
        a.remove(2, true);
        assert_eq!(tree(&a), "H(1 D)");
        a.remove(1, true);
        assert_eq!(tree(&a), "D");
    }

    #[test]
    fn minimise_and_restore_returns_the_window_to_its_spot() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let t2 = a.dock(2, t1, Edge::Bottom, None);
        a.dock(3, t2, Edge::Right, None);
        let shape = tree(&a);
        let before = all_weights(&a);
        let target = a.remove(3, true).unwrap();
        assert_ne!(tree(&a), shape);
        a.restore(3, target);
        assert_eq!(tree(&a), shape);
        assert!(all_weights(&a).iter().zip(&before).all(|(x, y)| near_all(x, y)));
    }

    #[test]
    fn restoring_the_first_child_of_a_split_puts_it_back_first() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        let target = a.remove(1, true).unwrap();
        a.restore(1, target);
        assert_eq!(tree(&a), "H(1 2 D)");
    }

    #[test]
    fn restore_falls_back_to_the_neighbours_window_when_its_node_is_gone() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let t2 = a.dock(2, t1, Edge::Bottom, None);
        a.dock(4, t2, Edge::Right, None); // H(V(1 H(2 4)) D)
        let target = a.remove(1, true).unwrap(); // beside the H(2 4) node, top
        a.remove(4, true); // that node collapses away
        assert!(a.restore(1, target).is_some());
        assert_eq!(tree(&a), "H(V(1 2) D)");
    }

    #[test]
    fn retaining_only_live_windows_drops_the_rest() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        a.dock(3, t1, Edge::Bottom, None);
        let gone: HashSet<WindowId> = a.retain_only(&HashSet::from([2]), true).into_iter().collect();
        assert_eq!(gone, HashSet::from([1, 3]));
        assert_eq!(tree(&a), "H(2 D)");
    }
}

mod tabs {
    use super::*;

    #[test]
    fn adding_a_tab_selects_it_windows_from_any_app_share_one_tile() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        assert_eq!(a.get(t).windows, [1, 2]);
        assert_eq!(a.selected_window(t), Some(2));
        assert_eq!(tree(&a), "H([1,2] D)");
    }

    #[test]
    fn removing_the_selected_tab_selects_the_next_one_or_the_previous_when_last() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        a.add_tab(3, t, None);
        a.select(2);
        a.remove(2, true);
        assert_eq!(a.selected_window(t), Some(3));
        a.remove(3, true);
        assert_eq!(a.selected_window(t), Some(1));
        assert_eq!(tree(&a), "H(1 D)");
    }

    #[test]
    fn removing_an_unselected_tab_keeps_the_selection() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        a.add_tab(3, t, None);
        a.remove(1, true);
        assert_eq!(a.selected_window(t), Some(3));
    }

    #[test]
    fn tearing_a_tab_off_and_restoring_it_puts_it_back_at_the_same_index() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        a.add_tab(3, t, None);
        let target = a.remove(2, true).unwrap();
        assert_eq!(target, RestoreTarget::Tab { window: 3, index: 1 });
        a.restore(2, target);
        assert_eq!(a.get(t).windows, [1, 2, 3]);
    }

    #[test]
    fn moving_a_whole_tab_group_keeps_its_tabs_together() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t1, None);
        let t3 = a.dock(3, t1, Edge::Right, None);
        a.move_node(t1, t3, Edge::Bottom, None);
        assert_eq!(tree(&a), "H(V(3 [1,2]) D)");
    }

    #[test]
    fn merging_one_group_into_another_makes_one_strip() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t1, None);
        let t3 = a.dock(3, t1, Edge::Right, None);
        a.merge(t1, t3);
        assert_eq!(a.get(t3).windows, [3, 1, 2]);
        assert_eq!(a.selected_window(t3), Some(2));
        assert_eq!(tree(&a), "H([3,1,2] D)");
    }
}

mod seams {
    use super::*;

    #[test]
    fn dragging_a_seam_resizes_both_neighbours_and_nothing_else() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        resize(&mut a, 0, 100.0, 900.0, SeamLimits::default());
        let w = weights(&a, a.root());
        assert!(near(w[0], 1.0 / 3.0 + 100.0 / 900.0));
        assert!(near(w[1], 1.0 / 3.0 - 100.0 / 900.0));
        assert!(near(w[2], 1.0 / 3.0));
    }

    #[test]
    fn a_seam_stops_where_a_window_would_refuse_to_shrink() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None); // H(1 2 D), thirds of 900 = 300 each
        let root = a.root();
        let after = |min_after| SeamLimits { min_after, ..SeamLimits::default() };
        // Window 2 can't go below 250 wide: dragging 200 right stops 50 in.
        resize(&mut a, 0, 200.0, 900.0, after(250.0));
        assert!(near(weights(&a, root)[1] * 900.0, 250.0));
        // A side that's already too small isn't squeezed further, but can grow.
        resize(&mut a, 0, 30.0, 900.0, after(400.0));
        assert!(near(weights(&a, root)[1] * 900.0, 250.0));
        resize(&mut a, 0, -30.0, 900.0, after(400.0));
        assert!(near(weights(&a, root)[1] * 900.0, 280.0));
    }

    #[test]
    fn a_subtrees_minimum_length_adds_along_its_axis_and_takes_the_largest_across() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Bottom, None); // H(V(1 2) D)
        let mins = HashMap::from([(1, Size::new(400.0, 300.0)), (2, Size::new(466.0, 252.0))]);
        let minimum = |w| mins.get(&w).copied();
        let v = a.get(a.root()).children[0];
        assert_eq!(a.minimum_length(v, Axis::Horizontal, 12.0, 60.0, &minimum), 466.0);
        assert_eq!(a.minimum_length(v, Axis::Vertical, 12.0, 60.0, &minimum), 300.0 + 12.0 + 252.0);
    }

    #[test]
    fn a_seam_cant_squeeze_a_neighbour_below_its_minimum() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        resize(&mut a, 0, 10_000.0, 900.0, SeamLimits::default());
        assert!(near(weights(&a, a.root())[1], 60.0 / 900.0));
    }
}

mod persistence {
    use super::*;

    #[test]
    fn an_arrangement_survives_a_json_round_trip() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(5, t1, None);
        a.dock(2, t1, Edge::Bottom, None);
        a.set_desktop_hidden(true);
        let mut nav = Navigator::new();
        nav.frame(a.parent(t1).unwrap(), &a);
        nav.save(3, &a);
        let highest = a.all_nodes(a.root()).into_iter().max().unwrap();
        let description = tree(&a);
        let windows = HashMap::from([
            (1, WindowRef::new("a", "one")),
            (2, WindowRef::new("b", "two")),
            (5, WindowRef::new("c", "five")),
        ]);
        let state = SavedState::new(BTreeMap::from([("UUID-1".to_string(), DisplayState::new(a, nav, windows))]));
        let decoded = SavedState::decode(&state.encoded().unwrap()).unwrap();
        let mut d = decoded.displays["UUID-1"].clone();
        assert_eq!(tree(&d.arrangement), description);
        assert!(d.arrangement.is_desktop_hidden());
        assert_eq!(d.arrangement.describe(d.navigator.framed_node(&d.arrangement)), "V([1,5] 2)");
        assert!(d.navigator.saved_views().contains_key(&3));
        // New nodes get fresh ids after decoding.
        let t = d.arrangement.dock_at_screen_edge(9, Edge::Right);
        let ids = d.arrangement.all_nodes(d.arrangement.root());
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(t > highest);
    }

    #[test]
    fn reads_the_files_the_swift_app_wrote() {
        // Written by the Swift version: a tab group and a nested split, a saved
        // range view, and a collapsed split's alias.
        let json = br#"{
          "version": 1,
          "displays": { "UUID-1|main": {
            "arrangement": {
              "aliases": { "7": 3 },
              "root": { "axis": "horizontal", "id": 5, "kind": "split", "weights": [0.25, 0.25, 0.5], "children": [
                { "id": 3, "kind": "tile", "selected": 1, "windows": [11, 12] },
                { "axis": "vertical", "id": 6, "kind": "split", "weights": [0.5, 0.5], "children": [
                  { "id": 4, "kind": "tile", "selected": 0, "windows": [13] },
                  { "id": 8, "kind": "tile", "selected": 0, "windows": [14] }
                ] },
                { "hidden": false, "id": 1, "kind": "desktop" }
              ] }
            },
            "navigator": {
              "backStack": [ { "includesDesktop": true, "isRoot": true, "node": 0, "windows": [] } ],
              "forwardStack": [],
              "framed": { "includesDesktop": false, "isRoot": false, "node": 6, "windows": [13, 14] },
              "savedViews": { "2": { "includesDesktop": false, "isRoot": false, "node": 5, "range": [3, 6], "windows": [11, 12, 13, 14] } }
            },
            "windows": {
              "11": { "bundleID": "a", "title": "one" }, "12": { "bundleID": "b", "title": "two" },
              "13": { "bundleID": "c", "title": "three" }, "14": { "bundleID": "d", "title": "four" }
            }
          } }
        }"#;
        let state = SavedState::decode(json).unwrap();
        let mut d = state.displays["UUID-1|main"].clone();
        assert_eq!(tree(&d.arrangement), "H([11,12] V(13 14) D)");
        assert_eq!(d.arrangement.selected_window(3), Some(12));
        assert_eq!(d.arrangement.node(7), Some(3), "aliases come back");
        assert_eq!(d.navigator.framed_node(&d.arrangement), 6);
        assert!(d.navigator.can_go_back());
        assert!(d.navigator.go_to(2, &d.arrangement));
        assert_eq!(d.navigator.framed_range(&d.arrangement), Some(vec![3, 6]));
        assert_eq!(d.window_refs()[&13], WindowRef::new("c", "three"));
        // And what Rust writes reads back the same.
        let again = SavedState::decode(&SavedState::new(state.displays.clone()).encoded().unwrap()).unwrap();
        assert_eq!(tree(&again.displays["UUID-1|main"].arrangement), "H([11,12] V(13 14) D)");
    }

    #[test]
    fn windows_are_matched_by_id_then_app_and_title_then_the_apps_only_window() {
        let saved = HashMap::from([
            (10, WindowRef::new("term", "zsh")),
            (11, WindowRef::new("safari", "News")),
            (12, WindowRef::new("notes", "Old title")),
            (13, WindowRef::new("mail", "Inbox")),
        ]);
        let live = HashMap::from([
            (10, WindowRef::new("term", "zsh — 80×24")), // same id, same app
            (90, WindowRef::new("safari", "News")),      // same app + title
            (91, WindowRef::new("notes", "New title")),  // only notes window
            (92, WindowRef::new("maps", "Inbox")),       // different app: no match
        ]);
        let m = WindowMatcher::match_windows(&saved, &live);
        assert_eq!(m, HashMap::from([(10, 10), (11, 90), (12, 91)]));
    }

    #[test]
    fn reattaching_drops_unmatched_windows_without_id_collisions() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        let mut state = DisplayState::new(
            a,
            Navigator::new(),
            HashMap::from([(1, WindowRef::new("x", "X")), (2, WindowRef::new("y", "Y"))]),
        );
        // Window 1 is gone; window "x" now has id 2 and "y" has id 7.
        state.reattach(&HashMap::from([(2, WindowRef::new("x", "X")), (7, WindowRef::new("y", "Y"))]), true);
        assert_eq!(tree(&state.arrangement), "H(2 7 D)");
    }

    #[test]
    fn preferences_fill_in_defaults_for_missing_keys() {
        let p: Preferences = serde_json::from_str(r#"{"gap": 20}"#).unwrap();
        assert_eq!(p.gap, 20.0);
        assert!(p.share_removed_space);
        assert_eq!(p.animation_duration, 0.46);
    }

    #[test]
    fn preferences_saved_before_version_2_get_the_shorter_tab_pause() {
        let old: Preferences = serde_json::from_str(r#"{"tabHold": 0.5}"#).unwrap();
        assert!(old.tab_hold == 0.3 && old.version == Preferences::CURRENT_VERSION);
        let chosen: Preferences = serde_json::from_str(r#"{"tabHold": 0.8}"#).unwrap();
        assert_eq!(chosen.tab_hold, 0.8);
        let current: Preferences = serde_json::from_str(r#"{"version": 2, "tabHold": 0.5}"#).unwrap();
        assert_eq!(current.tab_hold, 0.5);
    }

    #[test]
    fn preferences_keep_their_stored_key_names() {
        let json = serde_json::to_value(Preferences::default()).unwrap();
        for key in [
            "animate", "animationDuration", "coverTint", "gap", "greenButtonToggles", "ignoredBundleIDs",
            "livePreviews", "menuBarControls", "optionPinch", "rectZoom", "redrawWait", "shareRemovedSpace",
            "stepTowardPointer", "tabCloseButtons", "tabHold", "titleDoubleClickFrames", "version",
        ] {
            assert!(json.get(key).is_some(), "{key} is missing");
        }
    }

    #[test]
    fn shortcuts_keep_their_stored_form() {
        let stored: HashMap<String, Option<KeyCombo>> =
            serde_json::from_str(r#"{"canopy:\/\/save-view?n=1":{"modifiers":6656,"keyCode":18},"canopy:\/\/everything":null}"#).unwrap();
        let bindings = Shortcuts::from_stored(stored);
        assert_eq!(bindings[&Command::SaveView(1)], KeyCombo::new(18, 6656));
        assert!(!bindings.contains_key(&Command::Everything), "an unbound command stays unbound");
        assert_eq!(bindings[&Command::Back], Shortcuts::defaults()[&Command::Back]);
        let round = Shortcuts::from_stored(Shortcuts::to_stored(&bindings));
        assert_eq!(round, bindings);
    }
}
