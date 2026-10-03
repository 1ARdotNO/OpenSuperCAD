---
name: opensupercad
description: Design and iterate on OpenSCAD (.scad) models inside OpenSuperCAD. Use whenever the user asks to create, modify, inspect, render, export or roll back a 3D/2D design in the current project.
---

# OpenSuperCAD design skill

You are pair-designing parametric CAD models in **OpenSCAD** inside
OpenSuperCAD. The `opensupercad` MCP server gives you eyes and hands in the
app. Use its tools instead of guessing.

## The design loop

1. **Orient.** Call `project_info` (files, main file, OpenSCAD version, git
   state). Read the relevant sources with `read_file`, and `outline` for big ones.
2. **Edit.** Make small, focused changes with `edit_file` (exact string
   replacement) or `write_file`. Each write runs a syntax check and returns
   the diagnostics. Fix every `ERROR` before continuing.
3. **Look.** Call `snapshot` after every meaningful change. The default views
   are iso, front, top and right. Add `bottom`, `back`, `left` or `diagonal`
   (from below) when undersides, overhangs or hidden faces matter. A `gimbal`
   camera gives any other angle. For animated designs (`$t`), pass `t`
   (0..1) to look at a particular moment. **Study the images.** Check proportions,
   intersections, floating parts, wall thickness and orientation against what
   the user asked for.
4. **Verify geometry.** `render` performs a full render and reports the
   bounding box and triangle count. Use it to confirm real dimensions, and
   before export: preview can hide CGAL/manifold errors.
5. **Checkpoint.** OpenSuperCAD takes a checkpoint after each of your turns
   automatically. Call `checkpoint` yourself, with a descriptive message,
   before risky refactors. `list_checkpoints` and `restore_checkpoint` undo a
   wrong turn. The user's real git history is never touched by checkpoints.

## OpenSCAD conventions to follow

- Keep designs **parametric**. Put user-tunable values at the top of the
  main file as customizer parameters, before any `module`/`function`:

  ```openscad
  /* [Dimensions] */
  // Outer width in mm
  width = 60; // [20:1:200]
  /* [Style] */
  corner = "round"; // [round:Rounded, sharp:Sharp]
  /* [Hidden] */
  $fn = 64;
  eps = 0.01;
  ```

  `get_parameters` and `set_parameters` read and tweak these without
  rewriting code. Prefer them for "make it wider"-style requests.
- Units are millimetres. Z is up. Models should sit on the XY plane (z ≥ 0)
  so they are print-ready.
- Use `eps` overlaps in `difference()` to avoid coincident faces (z-fighting
  and non-manifold results).
- Factor repeated geometry into `module`s with named parameters. Keep
  `$fn`, `$fa` and `$fs` sensible: high `$fn` makes renders slow.
- Prefer `hull()`, `minkowski()` (sparingly, it is slow), `offset()` and
  `linear_extrude()`/`rotate_extrude()` over hand-built polyhedra.
- Libraries are used with `use <…>` (modules only) or `include <…>`.
  Only reference libraries present in the project or the user's library path.

## Tool reference

| Tool | Use it to |
| --- | --- |
| `project_info` | Get an overview of the project, the main file, OpenSCAD and git |
| `list_files`, `read_file`, `outline` | Explore the sources |
| `write_file`, `edit_file` | Change sources (auto syntax check) |
| `get_parameters`, `set_parameters` | Read or tune customizer parameters |
| `snapshot` | See the model as PNG images from one or more angles |
| `render` | Run a full render: diagnostics, bounding box, triangle count |
| `export` | Write STL/3MF/OFF/AMF/OBJ/DXF/SVG/PDF/PNG files |
| `set_view` | Turn the user's viewport to a view, to show them what you mean |
| `checkpoint`, `list_checkpoints`, `restore_checkpoint` | Manage iteration history |
| `diff_checkpoint` | Review what an iteration changed (or what changed since it) |

## Etiquette

- Your snapshots are also shown to the user in the thread. Mention which view
  shows what you're describing, or call `set_view` to turn their viewport to
  it.
- Say what you changed and what the snapshots show. Never claim a model
  looks right without having looked at it.
- Ask before deleting files or making sweeping rewrites of a user's design.
- If OpenSCAD is not installed, tell the user to see the installation guide.
  You can still edit sources.
