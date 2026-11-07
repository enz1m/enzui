//! The text grid's `Widget` implementation.

use super::*;

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for TextGridWidget<'_, Message>
where
    Theme: TextGridTheme,
    Renderer: text::Renderer,
    // the process-wide shape cache clones already-shaped paragraphs, so
    // the concrete paragraph type must be `Clone` (the graphics paragraph is a
    // cheap `Arc` clone) and `'static` (it is keyed in a thread-local by
    // `TypeId`). Every real iced renderer satisfies both.
    Renderer::Paragraph: Clone + 'static,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GridState<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(GridState::<Renderer::Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        let s = self.outer_size();
        // Fixed on both axes: the grid width is exactly cell-count * cell-width
        //  so rows lay flush and columns align.
        Size::new(Length::Fixed(s.width), Length::Fixed(s.height))
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let state = tree.state.downcast_mut::<GridState<Renderer::Paragraph>>();
        self.prepare(state, renderer);
        let size = limits.resolve(
            Length::Fixed(self.outer_size().width),
            Length::Fixed(self.outer_size().height),
            self.outer_size(),
        );
        layout::Node::new(size).apply(tree);
    }

    // An inert grid must not capture a press or write state, so
    // the whole body is skipped when nothing is wired.
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if self.is_inert() {
            return;
        }
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<GridState<Renderer::Paragraph>>();

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // A press another widget already claimed (a link, a scrollbar
                // thumb overlapping the row) is not ours.
                if shell.is_event_captured() {
                    return;
                }
                // `position_over` is the ownership test: only the
                // row the pointer is on starts a gesture, so a
                // column of rows reports exactly one press.
                let Some(at) = cursor.position_over(bounds) else {
                    return;
                };
                state.pressed = true;
                state.last_hover = None;
                if let Some(handler) = &self.on_press {
                    shell.publish(handler(self.pointer_at(at, bounds)));
                    // Captured because the grid now owns the gesture: without
                    // this a press that starts a selection would also reach
                    // whatever is underneath.
                    shell.capture_event();
                    shell.request_redraw();
                }
            }

            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                // The raw cursor, not `position_over`: a drag that leaves the
                // row must keep reporting, which is the whole point of the
                // saturating column and `row_offset`.
                let Some(at) = cursor.position() else {
                    return;
                };
                if state.pressed {
                    if let Some(handler) = &self.on_drag {
                        shell.publish(handler(self.pointer_at(at, bounds)));
                        shell.request_redraw();
                    }
                    return;
                }
                // Hover: inside only, and only on a column change, so a mouse
                // crossing a terminal does not publish once per pixel.
                let Some(handler) = &self.on_hover else {
                    return;
                };
                let inside = cursor.position_over(bounds).is_some();
                if !inside {
                    state.last_hover = None;
                    return;
                }
                let pointer = self.pointer_at(at, bounds);
                if state.last_hover != Some(pointer.column) {
                    state.last_hover = Some(pointer.column);
                    shell.publish(handler(pointer));
                }
            }

            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // Only the row that owns the live gesture reports the release,
                // wherever the pointer ended up.
                if !state.pressed {
                    return;
                }
                state.pressed = false;
                let Some(at) = cursor.position() else {
                    return;
                };
                if let Some(handler) = &self.on_release {
                    shell.publish(handler(self.pointer_at(at, bounds)));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }

            _ => {}
        }
    }

    // A wired grid reports `Interaction::Text` (the I-beam every
    // terminal shows); `cell_interaction` overrides per column so
    // e.g. an OSC-8 link can report `Pointer`. Inert grids report
    // `None`.
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let dragging = tree
            .state
            .downcast_ref::<GridState<Renderer::Paragraph>>()
            .pressed;
        let Some(at) = cursor.position_over(bounds) else {
            // A live drag keeps the affordance even once the pointer has left
            // the row, so the cursor does not flicker at the edge.
            return if dragging && self.on_drag.is_some() {
                mouse::Interaction::Text
            } else {
                mouse::Interaction::None
            };
        };
        if let Some(per_cell) = &self.cell_interaction {
            return per_cell(self.pointer_at(at, bounds).column);
        }
        if self.on_press.is_some() || self.on_drag.is_some() {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let palette = theme.enz_palette();
        let canvas_inks = self.canvas.map(|canvas| ToneInks::on(&palette, canvas));
        let mut background_inks = Vec::new();
        let cw = self.tokens.cell_width;
        let ch = self.tokens.cell_height;
        let scale = renderer_scale(renderer);

        // Snap in ancestor-offset space: the renderer applies the cumulative
        // transform after our submissions, so a fractional translation would
        // otherwise undo the snap and land cells off-pixel.
        // `current_transformation()` products every ancestor offset, outer
        // levels included.
        let ancestor_tx = renderer.current_transformation().translation().x;

        // The cache is populated in `layout`, but a redraw-only pass (no
        // relayout) still reaches here — the state is `&Tree`, so we cannot
        // reshape here. `Plain` paragraphs are already shaped from layout; if
        // the cache is somehow empty (never laid out), fall back to submitting
        // nothing rather than panicking on an index.
        let state = tree.state.downcast_ref::<GridState<Renderer::Paragraph>>();

        // Two row-wide passes, deliberately: glyphs may bleed into the next
        // column and double-width clusters own their continuation — a
        // per-cell background-then-glyph loop lets the next opaque background
        // erase legitimately painted ink. All backdrops first; glyphs and
        // decorations only onto the complete row.
        //
        // Adjacent cells with one background share one quad: a full-screen
        // TUI background is one quad per row, not one per cell.
        let fill_run = |renderer: &mut Renderer, from: usize, to: usize, bg: Color| {
            let x = snap_down(bounds.x + from as f32 * cw, scale, ancestor_tx);
            let end = snap_down(bounds.x + to as f32 * cw, scale, ancestor_tx);
            let rect = Rectangle {
                x,
                y: bounds.y,
                width: end - x,
                height: ch,
            };
            if rect.intersection(viewport).is_some() {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        snap: true,
                        ..Default::default()
                    },
                    Background::Color(bg),
                );
            }
        };
        let mut run: Option<(usize, Color)> = None;
        let mut col: usize = 0;
        for cell in self.cells.iter() {
            match (run, cell.bg) {
                (Some((_, open)), Some(bg)) if open == bg => {}
                (open, bg) => {
                    if let Some((from, colour)) = open {
                        fill_run(renderer, from, col, colour);
                    }
                    run = bg.map(|bg| (col, bg));
                }
            }
            col += cell.advance_cells();
        }
        if let Some((from, colour)) = run {
            fill_run(renderer, from, col, colour);
        }

        let mut col: usize = 0;
        for (idx, cell) in self.cells.iter().enumerate() {
            let span = cell.advance_cells();
            let cell_x = snap_down(bounds.x + col as f32 * cw, scale, ancestor_tx);
            let next_x = snap_down(bounds.x + (col + span) as f32 * cw, scale, ancestor_tx);
            let cell_rect = Rectangle {
                x: cell_x,
                y: bounds.y,
                width: next_x - cell_x,
                height: ch,
            };
            let bleed = self.tokens.glyph_paint_bleed_x(cell.grapheme.as_str());
            let glyph_end_x = snap_down(
                bounds.x + (col + cell.glyph_columns()) as f32 * cw,
                scale,
                ancestor_tx,
            );
            let glyph_rect = Rectangle {
                x: cell_x - bleed,
                y: bounds.y,
                width: (glyph_end_x - cell_x) + 2.0 * bleed,
                height: ch,
            };

            // Cull glyphs by their paint box, not their owning cell. A cell
            // just outside a horizontal viewport may still have wide/bleeding
            // ink inside it. Decorations remain cell-local below.
            if glyph_rect.intersection(viewport).is_some()
                && !cell.blank
                && !cell.grapheme.is_empty()
                && let Some(&Some(pi)) = state.order.get(idx)
                && let Some(para) = state.paragraphs.get(pi)
            {
                let raw = para.raw();
                let fg = resolve_fg_on(&palette, cell, canvas_inks.as_ref(), &mut background_inks);
                let anchor = glyph_rect.anchor(raw.min_bounds(), raw.align_x(), raw.align_y());
                if let Some(glyph_visible) = glyph_rect.intersection(viewport) {
                    renderer.fill_paragraph(raw, anchor, fg, glyph_visible);
                }
            }

            // Cursor/decorative ink is intentionally last so it stays visible
            // over both the row backgrounds and the marked glyph.
            if let Some(ink) = cell.decoration_ink
                && let Some(rect) = decoration_rect(cell.decoration, cell_rect, cw, ch)
                && rect.intersection(viewport).is_some()
            {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        snap: true,
                        ..Default::default()
                    },
                    Background::Color(ink),
                );
            }
            col += span;
        }
    }
}
