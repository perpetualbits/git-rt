// Verify the engine banner/selection: with the feature off, default is alacritty;
// RT_ENGINE=vtterm forces in-house. (The default-flip via feature is compile-time.)
#[test]
fn rt_engine_env_selects_vtterm() {
    std::env::set_var("RT_ENGINE", "vtterm");
    let pane = rt_engine::TermPane::spawn(
        Some(("/bin/sh".into(), vec!["-c".into(), "printf X".into()])), None, 20, 5).unwrap();
    assert!(matches!(pane, rt_engine::TermPane::Vt(_)), "RT_ENGINE=vtterm must pick the in-house engine");
}
#[test]
fn rt_engine_default_is_feature_gated() {
    std::env::remove_var("RT_ENGINE");
    let pane = rt_engine::TermPane::spawn(
        Some(("/bin/sh".into(), vec!["-c".into(), "printf X".into()])), None, 20, 5).unwrap();
    // Without the feature the default is alacritty; with it, vt-term.
    let is_vt = matches!(pane, rt_engine::TermPane::Vt(_));
    assert_eq!(is_vt, cfg!(feature = "vtterm-default"), "default must follow the build feature");
}
