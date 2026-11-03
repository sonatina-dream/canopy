//! The solver, planner, camera, drop resolution and URL commands (SPEC §2, §3,
//! §5, §6, §11).

use std::collections::{HashMap, HashSet};

use canopy_core::*;

const SCREEN: Rect = Rect::new(0.0, 25.0, 1512.0, 920.0);

fn tree(a: &Arrangement) -> String {
    a.describe(a.root())
}

fn framed(a: &Arrangement, nav: &Navigator) -> String {
    a.describe(nav.framed_node(a))
}

fn plan(a: &Arrangement, nav: &Navigator) -> Plan {
    Planner::plan(a, nav, SCREEN, 12.0, |_| None)
}

fn sorted(mut v: Vec<WindowId>) -> Vec<WindowId> {
    v.sort_unstable();
    v
}

fn visible_windows(p: &Plan) -> Vec<WindowId> {
    sorted(p.visible.keys().copied().collect())
}

mod solver {
    use super::*;

    #[test]
    fn children_share_a_split_by_weight_with_a_gap_between_and_around_them() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let bounds = Solver::bounds(SCREEN, 12.0);
        let l = Solver::solve(&a, a.root(), bounds, 12.0);
        let r1 = l.node_rects[&t1];
        let rd = l.node_rects[&a.desktop()];
        assert_eq!(r1.min_x(), 12.0);
        assert_eq!(r1.min_y(), 37.0);
        assert_eq!(rd.min_x() - r1.max_x(), 12.0);
        assert_eq!(rd.max_x(), 1500.0);
        assert!((r1.width - rd.width).abs() <= 1.0);
        assert_eq!(l.seams.len(), 1);
        assert_eq!(l.seams[0].rect, Rect::new(r1.max_x(), 37.0, 12.0, 896.0));
    }

    #[test]
    fn edges_are_whole_points_and_the_last_child_absorbs_rounding() {
        let mut a = Arrangement::new();
        let mut t = a.dock_at_screen_edge(1, Edge::Left);
        t = a.dock(2, t, Edge::Right, None);
        a.dock(3, t, Edge::Right, None);
        let l = Solver::solve(&a, a.root(), Rect::new(0.0, 0.0, 1001.0, 500.0), 7.0);
        let xs: Vec<Rect> = a.get(a.root()).children.iter().map(|c| l.node_rects[c]).collect();
        for r in &xs {
            assert!(r.min_x() == r.min_x().round() && r.max_x() == r.max_x().round());
        }
        assert_eq!(xs.last().unwrap().max_x(), 1001.0);
    }

    #[test]
    fn a_tab_group_reserves_a_strip_on_top_for_its_tabs() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        let l = Solver::solve(&a, a.root(), Rect::new(0.0, 0.0, 1000.0, 600.0), 0.0);
        assert_eq!(l.strip_rects[&t].height, Solver::TAB_STRIP_HEIGHT);
        assert_eq!(l.window_rects[&t].min_y(), Solver::TAB_STRIP_HEIGHT);
        assert_eq!(l.window_rects[&t].max_y(), 600.0);
    }

    #[test]
    fn a_hidden_desktop_gives_its_space_to_its_neighbours() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.set_desktop_hidden(true);
        let l = Solver::solve(&a, a.root(), Rect::new(0.0, 0.0, 1000.0, 600.0), 10.0);
        assert_eq!(l.node_rects[&t1], Rect::new(0.0, 0.0, 1000.0, 600.0));
        assert!(!l.node_rects.contains_key(&a.desktop()));
        assert!(l.seams.is_empty());
    }
}

mod planner {
    use super::*;

    #[test]
    fn windows_outside_the_framed_node_are_parked() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        let mut nav = Navigator::new();
        nav.frame(t1, &a);
        let p = plan(&a, &nav);
        assert_eq!(visible_windows(&p), [1]);
        assert_eq!(p.parked, HashSet::from([2]));
        assert_eq!(p.visible[&1], SCREEN.inset_by(12.0));
        assert_eq!(p.desktop_rect, None);
        // The out-of-view neighbour is projected off screen to its right.
        assert!(p.projected[&a.tile_of(2).unwrap()].min_x() >= SCREEN.max_x());
    }

    #[test]
    fn a_window_too_small_for_its_app_becomes_a_cover_until_theres_room() {
        let mut a = Arrangement::new();
        let mut t = a.dock_at_screen_edge(1, Edge::Left);
        for w in 2..=8 {
            t = a.dock(w, t, Edge::Right, None);
        }
        let mut nav = Navigator::new();
        let minimum = |_| Some(Size::new(400.0, 300.0));
        let p = Planner::plan(&a, &nav, SCREEN, 12.0, minimum);
        assert!(p.visible.is_empty());
        assert_eq!(p.covers.len(), 8);
        assert_eq!(p.parked.len(), 8);
        // Framing one window gives it room again.
        nav.frame(a.tile_of(3).unwrap(), &a);
        let q = Planner::plan(&a, &nav, SCREEN, 12.0, minimum);
        assert_eq!(visible_windows(&q), [3]);
    }

    #[test]
    fn a_window_a_few_points_bigger_than_its_tile_stays_a_window() {
        let m = Some(Size::new(466.0, 252.0));
        assert!(!Planner::is_cramped(Rect::new(0.0, 0.0, 464.0, 769.0), m));
        assert!(Planner::is_cramped(Rect::new(0.0, 0.0, 306.0, 769.0), m));
    }

    #[test]
    fn only_the_selected_tab_is_shown_the_others_are_parked() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        a.add_tab(3, t, None);
        a.select(2);
        let p = plan(&a, &Navigator::new());
        assert_eq!(visible_windows(&p), [2]);
        assert_eq!(p.parked, HashSet::from([1, 3]));
    }

    #[test]
    fn hidden_tabs_wait_behind_the_selected_one_at_its_frame() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        a.add_tab(3, t, None);
        a.select(2);
        let p = plan(&a, &Navigator::new());
        let wanted = StackedTab { rect: p.visible[&2], behind: 2 };
        assert_eq!(p.stacked, HashMap::from([(1, wanted), (3, wanted)]));
    }

    #[test]
    fn switching_tabs_keeps_every_window_at_the_same_frame() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        let before = plan(&a, &Navigator::new());
        a.select(1);
        let after = plan(&a, &Navigator::new());
        assert_eq!(before.visible[&2], after.visible[&1]);
        assert_eq!(before.stacked[&1].rect, after.stacked[&2].rect);
        assert_eq!(after.stacked[&2].behind, 1);
    }

    #[test]
    fn a_hidden_tab_too_big_for_its_group_is_parked_where_it_cant_stick_out() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        a.add_tab(3, t, None);
        a.select(1);
        let minimum = |w| (w == 3).then_some(Size::new(1400.0, 300.0));
        let p = Planner::plan(&a, &Navigator::new(), SCREEN, 12.0, minimum);
        assert_eq!(p.parked, HashSet::from([2, 3]));
        assert_eq!(p.stacked.keys().copied().collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn only_tabs_on_show_wait_behind_a_window() {
        let mut a = Arrangement::new();
        let t = a.dock_at_screen_edge(1, Edge::Left);
        a.add_tab(2, t, None);
        let t3 = a.dock(3, t, Edge::Right, None);
        let mut nav = Navigator::new();
        nav.frame(t3, &a);
        // Out of view: the whole group goes to the corner.
        let p = plan(&a, &nav);
        assert!(p.stacked.is_empty());
        assert_eq!(p.parked, HashSet::from([1, 2]));
        // A group shown as a cover hides its real windows, so nothing is on show.
        let q = Planner::plan(&a, &Navigator::new(), SCREEN, 12.0, |_| Some(Size::new(1400.0, 300.0)));
        assert!(q.stacked.is_empty());
    }
}

mod navigation {
    use super::*;

    /// Built so that there are levels to step through:
    /// root = H( V(1, H(2, 3)), D )
    fn world() -> (Arrangement, Navigator) {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let t2 = a.dock(2, t1, Edge::Bottom, None);
        a.dock(3, t2, Edge::Right, None);
        (a, Navigator::new())
    }

    fn rects(a: &Arrangement) -> HashMap<NodeId, Rect> {
        Solver::solve(a, a.root(), Solver::bounds(SCREEN, 12.0), 12.0).node_rects
    }

    #[test]
    fn step_in_goes_toward_the_focused_window_step_out_goes_back_up_a_level() {
        let (a, mut nav) = world();
        let r = rects(&a);
        assert_eq!(tree(&a), "H(V(1 H(2 3)) D)");
        assert!(nav.step_in(&a, Some(3), None, &r, false));
        assert_eq!(framed(&a, &nav), "V(1 H(2 3))");
        nav.step_in(&a, Some(3), None, &r, false);
        assert_eq!(framed(&a, &nav), "H(2 3)");
        nav.step_in(&a, Some(3), None, &r, false);
        assert_eq!(framed(&a, &nav), "3");
        assert!(!nav.step_in(&a, Some(3), None, &r, false), "a tile has nothing inside");
        nav.step_out(&a);
        assert_eq!(framed(&a, &nav), "H(2 3)");
        nav.everything(&a);
        assert_eq!(nav.framed_node(&a), a.root());
        assert!(!nav.step_out(&a), "nothing above everything");
    }

    #[test]
    fn step_in_can_go_toward_the_pointer_instead() {
        let (a, mut nav) = world();
        let r = rects(&a);
        let desktop_centre = r[&a.desktop()].center();
        nav.step_in(&a, Some(1), Some(desktop_centre), &r, true);
        assert!(a.get(nav.framed_node(&a)).is_desktop());
    }

    #[test]
    fn back_and_forward_work_like_a_web_browser() {
        let (a, mut nav) = world();
        let t3 = a.tile_of(3).unwrap();
        nav.frame(t3, &a);
        nav.step_out(&a);
        assert_eq!(framed(&a, &nav), "H(2 3)");
        nav.back(&a);
        assert_eq!(nav.framed_node(&a), t3);
        nav.back(&a);
        assert_eq!(nav.framed_node(&a), a.root());
        assert!(!nav.back(&a));
        nav.forward(&a);
        nav.forward(&a);
        assert_eq!(framed(&a, &nav), "H(2 3)");
        // A new move clears forward.
        nav.back(&a);
        nav.everything(&a);
        assert!(!nav.can_go_forward());
    }

    #[test]
    fn neighbouring_spaces_are_found_left_right_up_and_down_at_the_same_depth() {
        let (a, mut nav) = world();
        let r = rects(&a);
        nav.frame(a.tile_of(2).unwrap(), &a);
        nav.neighbour(Direction::Right, &a, &r);
        assert_eq!(framed(&a, &nav), "3");
        nav.neighbour(Direction::Up, &a, &r);
        assert_eq!(framed(&a, &nav), "1");
        nav.neighbour(Direction::Down, &a, &r);
        // 1 sits at depth 2; the space below it at that depth is H(2 3).
        assert_eq!(framed(&a, &nav), "H(2 3)");
        // From a deeper node, going up and down again lands at the same depth.
        nav.frame(a.tile_of(3).unwrap(), &a);
        nav.neighbour(Direction::Up, &a, &r);
        nav.neighbour(Direction::Down, &a, &r);
        assert_eq!(framed(&a, &nav), "H(2 3)", "1 is a leaf, so down from it is its sibling space");
        nav.frame(a.tile_of(1).unwrap(), &a);
        nav.neighbour(Direction::Right, &a, &r);
        assert!(a.get(nav.framed_node(&a)).is_desktop());
        assert!(!nav.neighbour(Direction::Right, &a, &r), "nothing right of the Desktop");
    }

    #[test]
    fn frame_window_toggles_again_goes_back() {
        let (a, mut nav) = world();
        let t2 = a.tile_of(2).unwrap();
        nav.toggle_frame(t2, &a);
        assert_eq!(nav.framed_node(&a), t2);
        nav.toggle_frame(t2, &a);
        assert_eq!(nav.framed_node(&a), a.root());
    }

    #[test]
    fn saved_views_1_to_9_come_straight_back_from_anywhere() {
        let (a, mut nav) = world();
        nav.frame(a.parent(a.tile_of(2).unwrap()).unwrap(), &a);
        nav.save(4, &a);
        nav.save(10, &a);
        assert_eq!(nav.saved_slots(), [4]);
        nav.everything(&a);
        assert!(nav.go_to(4, &a));
        assert_eq!(framed(&a, &nav), "H(2 3)");
        assert!(!nav.go_to(5, &a));
        nav.forget(4);
        assert!(nav.saved_views().is_empty());
    }

    #[test]
    fn a_saved_view_still_resolves_after_its_node_is_gone() {
        let (mut a, mut nav) = world();
        nav.frame(a.parent(a.tile_of(2).unwrap()).unwrap(), &a); // H(2 3)
        nav.save(1, &a);
        a.dock(4, a.tile_of(2).unwrap(), Edge::Bottom, None); // H(V(2 4) 3): H(2 3) node is still there
        a.remove(3, true); // H(2 3) collapses into V(2 4)
        nav.everything(&a);
        nav.go_to(1, &a);
        assert_eq!(framed(&a, &nav), "V(2 4)", "the view follows the node that took its place");
        // A view whose node was removed outright falls back to its windows.
        nav.frame(a.tile_of(4).unwrap(), &a);
        nav.save(2, &a);
        a.add_tab(4, a.tile_of(1).unwrap(), None);
        nav.everything(&a);
        nav.go_to(2, &a);
        assert_eq!(framed(&a, &nav), "[1,4]");
    }

    #[test]
    fn zoom_to_a_rectangle_frames_what_it_covers() {
        let (a, mut nav) = world();
        let r = rects(&a);
        let r2 = r[&a.tile_of(2).unwrap()];
        let r3 = r[&a.tile_of(3).unwrap()];
        nav.frame_rect(r2.union(&r3).inset_by(30.0), &a, &r);
        assert_eq!(framed(&a, &nav), "H(2 3)");
        nav.frame_rect(r2.inset_by(40.0), &a, &r);
        assert_eq!(framed(&a, &nav), "2");
        // A sliver over a neighbour doesn't pull it in.
        nav.everything(&a);
        let sliver = Rect::new(r2.min_x() + 20.0, r2.min_y() + 20.0, r2.width - 20.0 + 14.0, r2.height - 40.0);
        nav.frame_rect(sliver, &a, &r);
        assert_eq!(framed(&a, &nav), "2");
    }

    #[test]
    fn a_rectangle_over_some_of_a_row_frames_just_that_run_of_windows() {
        let mut a = Arrangement::new();
        let mut t = a.dock_at_screen_edge(1, Edge::Left);
        t = a.dock(2, t, Edge::Right, None);
        a.dock(3, t, Edge::Right, None); // H(1 2 3 D)
        let mut nav = Navigator::new();
        let r = rects(&a);
        let r1 = r[&a.tile_of(1).unwrap()];
        let r2 = r[&a.tile_of(2).unwrap()];
        assert!(nav.frame_rect(r1.union(&r2).inset_by(40.0), &a, &r));
        let describe = |nav: &Navigator, a: &Arrangement| {
            nav.framed_range(a).map(|run| run.iter().map(|&n| a.describe(n)).collect::<Vec<_>>())
        };
        assert_eq!(describe(&nav, &a), Some(vec!["1".to_string(), "2".to_string()]));
        // The run fills the screen; everything else is out of view.
        let p = plan(&a, &nav);
        assert_eq!(visible_windows(&p), [1, 2]);
        assert_eq!(p.parked, HashSet::from([3]));
        assert!((p.visible[&1].width - p.visible[&2].width).abs() <= 1.0);
        assert!(p.visible[&1].min_x() == 12.0 && p.visible[&2].max_x() == SCREEN.max_x() - 12.0);
        // Dragging the run's gap resizes those two in the real split.
        let seam = *p.layout.seams.first().unwrap();
        assert!(seam.split == a.root() && seam.index == 0 && seam.next == 1);
        a.resize_seam(a.root(), seam.index, seam.next, 100.0, seam.available, SeamLimits::default());
        let q = plan(&a, &nav);
        assert!((q.visible[&1].width - (p.visible[&1].width + 100.0)).abs() <= 1.0);
        // Neighbour right of the run is 3; stepping out frames the whole row.
        assert_eq!(nav.neighbour_node(Direction::Right, &a, &r).map(|n| a.describe(n)), Some("3".to_string()));
        nav.step_out(&a);
        assert!(nav.framed_range(&a).is_none() && nav.framed_node(&a) == a.root());
        // Back returns to the run, and a saved run comes back too.
        nav.back(&a);
        assert_eq!(nav.framed_range(&a).map(|r| r.len()), Some(2));
        nav.save(2, &a);
        nav.everything(&a);
        nav.go_to(2, &a);
        assert_eq!(describe(&nav, &a), Some(vec!["1".to_string(), "2".to_string()]));
    }

    #[test]
    fn a_pinch_settles_on_the_level_that_matches_how_far_you_pinched() {
        let (a, mut nav) = world(); // H(V(1 H(2 3)) D)
        let bounds = Solver::bounds(SCREEN, 12.0);
        let t3 = a.tile_of(3).unwrap();
        let projected = |nav: &Navigator| plan(&a, nav).projected;
        let p3 = projected(&nav)[&t3].center();
        // A small pinch stays put.
        assert_eq!(nav.settle_target(1.05, p3, &a, &projected(&nav), bounds), a.root());
        // Pinching in a lot over 3 settles on 3; a little, on a level above it.
        assert_eq!(nav.settle_target(8.0, p3, &a, &projected(&nav), bounds), t3);
        let mid = nav.settle_target(2.2, p3, &a, &projected(&nav), bounds);
        assert!(a.node_contains(mid, t3) && mid != a.root() && mid != t3);
        // From 3, pinching out a lot settles on everything.
        nav.frame(t3, &a);
        assert_eq!(nav.settle_target(0.15, p3, &a, &projected(&nav), bounds), a.root());
        // Pinching in over the Desktop settles on it.
        nav.everything(&a);
        let pd = projected(&nav)[&a.desktop()].center();
        assert_eq!(nav.settle_target(2.2, pd, &a, &projected(&nav), bounds), a.desktop());
    }

    #[test]
    fn reveal_widens_the_view_so_a_newly_docked_window_is_never_out_of_sight() {
        let (mut a, mut nav) = world();
        let t1 = a.tile_of(1).unwrap();
        nav.frame(t1, &a);
        let t9 = a.dock(9, t1, Edge::Right, None);
        assert!(nav.reveal(t9, &a));
        assert_eq!(framed(&a, &nav), "H(1 9)");
    }

    #[test]
    fn bringing_a_cramped_window_into_view_steps_down_until_it_fits() {
        let mut a = Arrangement::new();
        let mut t = a.dock_at_screen_edge(1, Edge::Left);
        for w in 2..=6 {
            t = a.dock(w, t, Edge::Bottom, None);
        }
        // Six windows stacked: each ~140 pt tall, under 300 pt minimum.
        let mut nav = Navigator::new();
        let min = Some(Size::new(300.0, 300.0));
        let t4 = a.tile_of(4).unwrap();
        nav.bring_into_view(t4, &a, |framing| Planner::fits(&a, t4, framing, SCREEN, 12.0, min));
        assert_eq!(nav.framed_node(&a), t4);
    }
}

mod drops {
    use super::*;

    const TILE: Rect = Rect::new(12.0, 37.0, 738.0, 896.0);
    const DESKTOP: Rect = Rect::new(762.0, 37.0, 738.0, 896.0);

    fn resolver() -> DropResolver {
        DropResolver::default()
    }

    #[test]
    fn near_the_screen_edge_docks_beside_everything() {
        let t = resolver().resolve(Point::new(5.0, 400.0), SCREEN, None, &[(5, TILE)], Some((1, DESKTOP)), None);
        assert_eq!(t, DropTarget::ScreenEdge(Edge::Left));
        let b = resolver().resolve(Point::new(1000.0, 940.0), SCREEN, None, &[], Some((1, DESKTOP)), None);
        assert_eq!(b, DropTarget::ScreenEdge(Edge::Bottom));
    }

    #[test]
    fn pushing_the_pointer_into_the_menu_bar_or_the_dock_is_the_screen_edge() {
        let display = Some(Rect::new(0.0, 0.0, 1512.0, 982.0));
        let top = resolver().resolve(Point::new(700.0, 10.0), SCREEN, display, &[(5, TILE)], Some((1, DESKTOP)), None);
        assert_eq!(top, DropTarget::ScreenEdge(Edge::Top));
        let dock = resolver().resolve(Point::new(700.0, 970.0), SCREEN, display, &[(5, TILE)], Some((1, DESKTOP)), None);
        assert_eq!(dock, DropTarget::ScreenEdge(Edge::Bottom));
        let elsewhere = resolver().resolve(Point::new(3000.0, 10.0), SCREEN, display, &[], None, None);
        assert_eq!(elsewhere, DropTarget::None, "another display's pointer is not this display's edge");
    }

    #[test]
    fn over_a_windows_edge_docks_beside_it_on_that_edge() {
        let at = |u: f64, v: f64| {
            let p = Point::new(TILE.min_x() + u * TILE.width, TILE.min_y() + v * TILE.height);
            resolver().resolve(p, SCREEN, None, &[(5, TILE)], Some((1, DESKTOP)), None)
        };
        assert_eq!(at(0.2, 0.5), DropTarget::Beside(5, Edge::Left));
        assert_eq!(at(0.9, 0.5), DropTarget::Beside(5, Edge::Right));
        assert_eq!(at(0.5, 0.1), DropTarget::Beside(5, Edge::Top));
        assert_eq!(at(0.5, 0.85), DropTarget::Beside(5, Edge::Bottom));
        assert_eq!(at(0.15, 0.2), DropTarget::Beside(5, Edge::Left), "nearest edge wins in a corner");
    }

    #[test]
    fn on_a_big_window_the_edge_zones_stop_at_200_pt_so_most_of_it_makes_tabs() {
        let big = Rect::new(12.0, 37.0, 1400.0, 880.0);
        let r = resolver();
        assert_eq!(r.zone(Point::new(big.min_x() + 0.3 * big.width, big.mid_y()), big), Zone::Centre);
        assert_eq!(r.zone(Point::new(big.min_x() + 150.0, big.mid_y()), big), Zone::Edge(Edge::Left));
        assert_eq!(r.zone(Point::new(big.mid_x(), big.min_y() + 0.15 * big.height), big), Zone::Edge(Edge::Top));
        assert_eq!(r.zone(Point::new(big.mid_x(), big.min_y() + 0.30 * big.height), big), Zone::Centre);
    }

    #[test]
    fn the_middle_of_a_window_makes_tabs_only_after_a_pause() {
        let p = TILE.center();
        assert_eq!(resolver().resolve(p, SCREEN, None, &[(5, TILE)], None, None), DropTarget::None);
        assert_eq!(resolver().resolve(p, SCREEN, None, &[(5, TILE)], None, Some(5)), DropTarget::Tab(5));
    }

    #[test]
    fn over_the_desktop_floats_except_right_by_an_inner_edge() {
        let r = resolver();
        assert_eq!(r.resolve(DESKTOP.center(), SCREEN, None, &[(5, TILE)], Some((1, DESKTOP)), None), DropTarget::Float);
        assert_eq!(
            r.resolve(Point::new(DESKTOP.min_x() + 10.0, 400.0), SCREEN, None, &[(5, TILE)], Some((1, DESKTOP)), None),
            DropTarget::Beside(1, Edge::Left)
        );
    }

    #[test]
    fn applying_drops_changes_the_arrangement_as_previewed() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        assert!(a.apply(DropTarget::Beside(t1, Edge::Bottom), 2, false));
        assert_eq!(tree(&a), "H(V(1 2) D)");
        assert!(a.apply(DropTarget::Tab(t1), 3, false));
        assert_eq!(tree(&a), "H(V([1,3] 2) D)");
        assert!(!a.apply(DropTarget::Float, 4, false));
        assert!(a.apply(DropTarget::ScreenEdge(Edge::Right), 1, true));
        assert_eq!(tree(&a), "H(2 D [1,3])");
    }

    fn two_docked() -> (Arrangement, Navigator, NodeId) {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        a.dock(2, t1, Edge::Right, None);
        (a, Navigator::new(), t1)
    }

    #[test]
    fn seeing_everything_a_screen_edge_docks_beside_everything() {
        let (a, nav, _) = two_docked();
        let edge = DropTarget::ScreenEdge(Edge::Left);
        assert_eq!(edge.in_view(9, false, &nav, &a), edge);
    }

    #[test]
    fn zoomed_in_a_screen_edge_docks_inside_what_is_in_view() {
        let (mut a, mut nav, t1) = two_docked();
        nav.frame(t1, &a);
        let target = DropTarget::ScreenEdge(Edge::Bottom).in_view(9, false, &nav, &a);
        assert_eq!(target, DropTarget::Beside(t1, Edge::Bottom));
        assert!(a.apply(target, 9, false));
        assert_eq!(tree(&a), "H(V(1 9) 2 D)", "the new window is nested in the space");
        nav.reveal(a.tile_of(9).unwrap(), &a);
        assert_eq!(framed(&a, &nav), "V(1 9)", "and the view is that space, not everything");
    }

    #[test]
    fn docking_along_the_row_keeps_the_rest_of_the_row_out_of_view() {
        let (mut a, mut nav, t1) = two_docked();
        nav.frame(t1, &a);
        let target = DropTarget::ScreenEdge(Edge::Right).in_view(9, false, &nav, &a);
        a.apply(target, 9, false);
        assert_eq!(tree(&a), "H(1 9 2 D)");
        let t9 = a.tile_of(9).unwrap();
        nav.reveal(t9, &a);
        assert_eq!(nav.framed_range(&a), Some(vec![t1, t9]));
        let p = plan(&a, &nav);
        assert_eq!(visible_windows(&p), vec![1, 9], "window 2 stays out of view");
        assert!((p.visible[&1].width - p.visible[&9].width).abs() <= 1.0, "they share the screen");

        // Another window goes on the end of the run, on whichever side it was dropped.
        let target = DropTarget::ScreenEdge(Edge::Right).in_view(8, false, &nav, &a);
        assert_eq!(target, DropTarget::Beside(t9, Edge::Right));
        let target = DropTarget::ScreenEdge(Edge::Left).in_view(8, false, &nav, &a);
        assert_eq!(target, DropTarget::Beside(t1, Edge::Left));
        a.apply(target, 8, false);
        nav.reveal(a.tile_of(8).unwrap(), &a);
        assert_eq!(tree(&a), "H(8 1 9 2 D)");
        assert_eq!(visible_windows(&plan(&a, &nav)), vec![1, 8, 9]);
    }

    #[test]
    fn across_a_run_a_screen_edge_docks_beside_its_whole_split() {
        let (a, mut nav, t1) = two_docked();
        let t2 = a.tile_of(2).unwrap();
        nav.frame_range(&[t1, t2], &a);
        let edge = DropTarget::ScreenEdge(Edge::Top);
        assert_eq!(edge.in_view(9, false, &nav, &a), edge, "that split is the root here");
    }

    #[test]
    fn a_window_that_is_the_whole_view_docks_beside_everything_as_before() {
        let (a, mut nav, t1) = two_docked();
        nav.frame(t1, &a);
        let edge = DropTarget::ScreenEdge(Edge::Right);
        assert_eq!(edge.in_view(1, true, &nav, &a), edge);
    }

    #[test]
    fn dragging_a_window_out_of_a_framed_split_keeps_both_in_view() {
        let (mut a, mut nav, t1) = two_docked();
        a.dock(3, t1, Edge::Bottom, None);
        assert_eq!(tree(&a), "H(V(1 3) 2 D)");
        let split = a.parent(t1).unwrap();
        nav.frame(split, &a);
        let target = DropTarget::ScreenEdge(Edge::Right).in_view(1, true, &nav, &a);
        assert_eq!(target, DropTarget::Beside(split, Edge::Right));
        a.apply(target, 1, true);
        assert_eq!(tree(&a), "H(3 1 2 D)");
        nav.reveal(t1, &a);
        assert_eq!(visible_windows(&plan(&a, &nav)), vec![1, 3]);
    }

    #[test]
    fn a_window_next_to_the_view_never_makes_a_run_of_the_whole_split() {
        let mut a = Arrangement::new();
        let t1 = a.dock_at_screen_edge(1, Edge::Left);
        let mut nav = Navigator::new();
        nav.frame(t1, &a);
        let t9 = a.dock(9, t1, Edge::Bottom, None);
        nav.reveal(t9, &a);
        assert_eq!(nav.framed_range(&a), None);
        assert_eq!(framed(&a, &nav), "V(1 9)");
    }

    const FLOATING: Rect = Rect::new(100.0, 100.0, 800.0, 600.0);
    const WIDE_DESK: Rect = Rect::new(12.0, 37.0, 1488.0, 896.0);

    /// What a drop at `p` on floating window 5 leaves, on an empty Desktop.
    fn joined(p: Point, desk: Rect) -> String {
        let mut a = Arrangement::new();
        let target = resolver().over_floating(p, 5, FLOATING, (a.desktop(), desk));
        assert!(target.docks());
        assert!(a.apply(target, 6, false));
        tree(&a)
    }

    #[test]
    fn dropping_on_a_floating_windows_edge_docks_them_side_by_side() {
        let (mid_x, mid_y) = (FLOATING.mid_x(), FLOATING.mid_y());
        assert_eq!(joined(Point::new(FLOATING.max_x() - 20.0, mid_y), WIDE_DESK), "H(5 6 D)");
        assert_eq!(joined(Point::new(FLOATING.min_x() + 20.0, mid_y), WIDE_DESK), "H(6 5 D)");
        assert_eq!(joined(Point::new(mid_x, FLOATING.min_y() + 20.0), WIDE_DESK), "H(V(6 5) D)");
        assert_eq!(joined(Point::new(mid_x, FLOATING.max_y() - 20.0), WIDE_DESK), "H(V(5 6) D)");
    }

    #[test]
    fn dropping_in_a_floating_windows_middle_makes_tabs() {
        assert_eq!(joined(FLOATING.center(), WIDE_DESK), "H([5,6] D)");
    }

    #[test]
    fn a_floating_window_docks_on_the_side_of_the_desktop_it_was_on() {
        let right = Rect::new(-1000.0, 37.0, 1600.0, 896.0);
        assert_eq!(joined(FLOATING.center(), right), "H(D [5,6])");
        let tall = Rect::new(100.0, 37.0, 600.0, 900.0);
        assert_eq!(joined(FLOATING.center(), tall), "V([5,6] D)");
    }

    #[test]
    fn a_docked_window_dropped_on_a_floating_one_moves_there_whole() {
        let mut a = Arrangement::new();
        a.dock_at_screen_edge(1, Edge::Left);
        let target = DropTarget::Floating { window: 5, beside: a.desktop(), side: Edge::Right, edge: Some(Edge::Bottom) };
        assert!(a.apply(target, 1, true));
        assert_eq!(tree(&a), "H(D V(5 1))");
    }
}

mod commands {
    use super::*;

    #[test]
    fn every_documented_url_parses() {
        for (raw, expected) in [
            ("canopy://everything", Command::Everything),
            ("canopy://zoom-out", Command::StepOut),
            ("canopy://zoom-in", Command::StepIn),
            ("canopy://back", Command::Back),
            ("canopy://forward", Command::Forward),
            ("canopy://neighbour?dir=left", Command::Neighbour(Direction::Left)),
            ("canopy://neighbor?dir=DOWN", Command::Neighbour(Direction::Down)),
            ("canopy://frame-window", Command::FrameWindow),
            ("canopy://frame-desktop", Command::FrameDesktop),
            ("canopy://toggle-desktop", Command::ToggleDesktop),
            ("canopy://view?n=1", Command::GoToView(1)),
            ("canopy://save-view?n=9", Command::SaveView(9)),
            ("canopy://split-finder", Command::SplitFinder),
            ("canopy://arrange", Command::Arrange),
            ("canopy://float-all", Command::FloatAll),
            ("canopy://settings?pane=shortcuts", Command::Settings(Some(SettingsPane::Shortcuts))),
            ("canopy://settings", Command::Settings(None)),
            ("canopy://settings?pane=nope", Command::Settings(None)),
            ("CANOPY://Everything", Command::Everything),
            ("canopy:everything", Command::Everything),
            ("canopy:///everything", Command::Everything),
        ] {
            assert_eq!(Command::from_url(raw), Some(expected), "{raw}");
        }
    }

    #[test]
    fn invalid_urls_are_ignored() {
        for raw in [
            "canopy://view",
            "canopy://view?n=0",
            "canopy://view?n=10",
            "canopy://view?n=two",
            "canopy://save-view?n=-1",
            "canopy://neighbour",
            "canopy://neighbour?dir=diagonal",
            "canopy://launch-missiles",
            "other://everything",
            "canopy://",
        ] {
            assert_eq!(Command::from_url(raw), None, "{raw}");
        }
    }

    #[test]
    fn every_commands_url_round_trips() {
        let mut all = vec![
            Command::Everything,
            Command::StepOut,
            Command::StepIn,
            Command::Back,
            Command::Forward,
            Command::FrameWindow,
            Command::FrameDesktop,
            Command::ToggleDesktop,
            Command::SplitFinder,
            Command::Arrange,
            Command::FloatAll,
            Command::Settings(None),
        ];
        all.extend(Direction::ALL.map(Command::Neighbour));
        all.extend((1..=9).flat_map(|n| [Command::GoToView(n), Command::SaveView(n)]));
        all.extend(SettingsPane::ALL.map(|p| Command::Settings(Some(p))));
        for c in all {
            assert_eq!(Command::from_url(&c.url()), Some(c), "{}", c.url());
        }
    }
}
