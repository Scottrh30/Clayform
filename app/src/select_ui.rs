//! Masking and Visibility panels: lasso/rectangle masks, mask tools,
//! hiding, Clip / Trim / Slice curves and Extract.

use crate::{act, with_ed, Action, Check, Folder, Slider, Ui};
use clayform_core::editor::{CutTool, Editor, PickAction, SelectShape};
use clayform_core::sculpt::Tool;
use leptos::prelude::*;

/// Runs an editor action, redraws and shows its message (or `done`).
fn run(ui: Ui, f: impl FnOnce(&mut Editor) -> Result<Option<String>, String>) {
    let r = with_ed(|e| {
        e.cancel_drag();
        let r = f(e);
        e.redraw = true;
        r
    });
    ui.refresh();
    match r {
        Ok(Some(m)) | Err(m) => ui.status.set(m),
        Ok(None) => {}
    }
}

/// Keyboard-shortcut commands.
pub fn command(ui: Ui, id: &str) {
    match id {
        "grow_mask" => run(ui, |e| changed(e.sculptor.grow_mask(), "Nothing is masked")),
        "shrink_mask" => run(ui, |e| changed(e.sculptor.shrink_mask(), "Nothing is masked")),
        "blur_mask" => run(ui, |e| changed(e.sculptor.blur_mask(), "Nothing is masked")),
        "sharpen_mask" => run(ui, |e| changed(e.sculptor.sharpen_mask(), "Nothing to sharpen")),
        "show_all" => run(ui, |e| changed(e.sculptor.show_all(), "Nothing is hidden")),
        "invert_visibility" => run(ui, |e| e.sculptor.invert_visibility().map(|_| None)),
        _ => {}
    }
}

fn changed(b: bool, none: &str) -> Result<Option<String>, String> {
    if b { Ok(None) } else { Ok(Some(none.to_string())) }
}

fn arm(ui: Ui, a: PickAction, what: &str) {
    with_ed(|e| e.pending_pick = Some(a));
    ui.mode.set(clayform_core::editor::Mode::Sculpt);
    ui.status.set(format!("Click a polygroup on the model to {what}"));
}

#[component]
fn ShapePicker(ui: Ui, shape: RwSignal<SelectShape>) -> impl IntoView {
    let set = move |s: SelectShape| {
        shape.set(s);
        with_ed(|e| e.select_shape = s);
    };
    let _ = ui;
    view! {
        <div class="row">
            <span class="label">"Shape"</span>
            <div class="segmented">
                <button class:on=move || shape.get() == SelectShape::Lasso on:click=move |_| set(SelectShape::Lasso)>"Lasso"</button>
                <button class:on=move || shape.get() == SelectShape::Rect on:click=move |_| set(SelectShape::Rect)>"Rectangle"</button>
            </div>
        </div>
    }
}

#[component]
pub fn MaskingFolder(ui: Ui, shape: RwSignal<SelectShape>) -> impl IntoView {
    let strength = RwSignal::new(0.6f32);
    view! {
        <Folder title="Masking">
            <div class="row buttons">
                <button on:click=move |_| ui.tool.set(Tool::Mask)>"Mask brush"</button>
                <button on:click=move |_| act(ui, Action::ClearMask)>"Clear"</button>
                <button on:click=move |_| act(ui, Action::InvertMask)>"Invert"</button>
            </div>
            <ShapePicker ui shape />
            <div class="row info">"Ctrl + drag off the model masks inside the outline (mirrored by symmetry), Ctrl + Alt + drag unmasks. Ctrl + click off the model inverts the mask; Ctrl + Alt + click clears it."</div>
            <div class="row buttons tight">
                <button title="Spread the mask one ring of points" on:click=move |_| run(ui, |e| changed(e.sculptor.grow_mask(), "Nothing is masked"))>"Grow"</button>
                <button title="Pull the mask back one ring of points" on:click=move |_| run(ui, |e| changed(e.sculptor.shrink_mask(), "Nothing is masked"))>"Shrink"</button>
                <button title="Soften the mask's edge" on:click=move |_| run(ui, |e| changed(e.sculptor.blur_mask(), "Nothing is masked"))>"Blur"</button>
                <button title="Harden the mask's edge" on:click=move |_| run(ui, |e| changed(e.sculptor.sharpen_mask(), "Nothing to sharpen"))>"Sharpen"</button>
            </div>
            <Slider label="Strength" min=0.05 max=1.0 step=0.01 value=strength on_change=move |v| strength.set(v) />
            <div class="row buttons tight">
                <button title="Mask the creases and hollows" on:click=move |_| {
                    let s = strength.get_untracked();
                    run(ui, move |e| changed(e.sculptor.mask_by_curvature(false, s), "The surface has no hollows"))
                }>"By cavity"</button>
                <button title="Mask the ridges and bumps" on:click=move |_| {
                    let s = strength.get_untracked();
                    run(ui, move |e| changed(e.sculptor.mask_by_curvature(true, s), "The surface has no peaks"))
                }>"By peaks"</button>
                <button title="Click a polygroup: everything else is masked" on:click=move |_| arm(ui, PickAction::MaskGroup, "keep unmasked")>"By polygroup"</button>
            </div>
            <Check
                label="Show mask"
                value=Signal::derive(move || ui.view.get().show_mask)
                on_change=move |v| ui.view.update(|s| s.show_mask = v)
            />
        </Folder>
    }
}

#[component]
pub fn VisibilityFolder(ui: Ui, shape: RwSignal<SelectShape>) -> impl IntoView {
    let tool = RwSignal::new(CutTool::Select);
    let thick = RwSignal::new(0.02f32);
    let smooth = RwSignal::new(3f32);
    let set_tool = move |t: CutTool| {
        tool.set(t);
        with_ed(|e| e.cut_tool = t);
        ui.mode.set(clayform_core::editor::Mode::Sculpt);
    };
    let hint = move || match tool.get() {
        CutTool::Select => "Ctrl + Shift + drag shows only what's inside the outline; add Alt to hide it instead. Ctrl + Shift + click a polygroup to isolate it (with Alt: hide it); click off the model to show everything.",
        CutTool::Clip => "Ctrl + Shift + drag a line: everything on the ticked side is flattened onto it (masked points stay). Alt flips the side. Uses symmetry.",
        CutTool::Trim => "Ctrl + Shift + drag a line: the ticked side is cut away and the hole capped flat. Alt flips the side. Uses symmetry.",
        CutTool::Slice => "Ctrl + Shift + drag a line: the faces are split along it and the ticked side becomes new polygroups. Alt flips the side.",
    };
    view! {
        <Folder title="Visibility and cuts" closed=true>
            <div class="row">
                <div class="segmented">
                    {CutTool::ALL
                        .iter()
                        .map(|t| {
                            let t = *t;
                            view! { <button class:on=move || tool.get() == t on:click=move |_| set_tool(t)>{t.name().split(' ').next().unwrap_or("")}</button> }
                        })
                        .collect_view()}
                </div>
            </div>
            <Show when=move || tool.get() == CutTool::Select>
                <ShapePicker ui shape />
            </Show>
            <div class="row info">{hint}</div>
            <div class="subhead">"Hide and show"</div>
            <div class="row buttons tight">
                <button on:click=move |_| run(ui, |e| changed(e.sculptor.show_all(), "Nothing is hidden"))>"Show all"</button>
                <button on:click=move |_| run(ui, |e| e.sculptor.invert_visibility().map(|_| None))>"Invert"</button>
                <button title="Hide the faces that are masked" on:click=move |_| run(ui, |e| e.sculptor.hide_by_mask(true).and_then(|b| changed(b, "Nothing is masked")))>"Hide masked"</button>
                <button title="Hide the faces that aren't masked" on:click=move |_| run(ui, |e| e.sculptor.hide_by_mask(false).and_then(|b| changed(b, "Nothing to hide")))>"Hide unmasked"</button>
            </div>
            <div class="row buttons tight">
                <button title="Click a polygroup to hide it" on:click=move |_| arm(ui, PickAction::HideGroup, "hide it")>"Hide polygroup"</button>
                <button title="Click a polygroup to show only it" on:click=move |_| arm(ui, PickAction::IsolateGroup, "show only it")>"Isolate polygroup"</button>
                <button title="Remove the hidden faces for good" on:click=move |_| run(ui, |e| e.sculptor.delete_hidden().map(|n| Some(format!("Deleted {n} hidden triangles"))))>"Delete hidden"</button>
            </div>
            <div class="row info">"Hidden parts aren't drawn, picked or sculpted, and stay hidden on every subdivision level. They're saved with the project and still exported."</div>
            <div class="subhead">"Extract"</div>
            <Slider label="Thickness" min=0.002 max=0.15 step=0.001 value=thick on_change=move |v| thick.set(v)
                format=|v| format!("{:.1}%", v * 100.0) />
            <Slider label="Smooth edge" min=0.0 max=8.0 step=1.0 value=smooth on_change=move |v| smooth.set(v)
                format=|v| format!("{v:.0}") />
            <div class="row buttons">
                <button class="primary" on:click=move |_| {
                    let (t, s) = (thick.get_untracked(), smooth.get_untracked() as u32);
                    run(ui, move |e| e.extract(t, s).map(|_| Some("Extracted the masked area as a new SubTool".into())));
                }>"Extract (new SubTool)"</button>
            </div>
            <div class="row info">"Turns the masked area into a closed shell grown out from the surface, for clothing, armour or straps. Thickness is a share of the model's size."</div>
        </Folder>
    }
}
