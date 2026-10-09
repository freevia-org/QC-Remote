/**
 * The name CorOS draws for a scene, as opposed to the label the preset stores.
 *
 * A scene with no custom label is delivered to the app as "Scene A".."Scene H"
 * (the Rust state decoder and the gateway both fill the gap that way, and
 * set_scene_label's expected-label guard compares against that spelling, so the
 * snapshot keeps it). The unit itself draws the first scene's empty label as
 * "Default scene" (corpus: grid-scene-selector.tree.txt and
 * gig-view-scene.tree.txt, SceneTableCell text) and every other empty scene as
 * "Scene B".."Scene H". This maps only what is drawn; it never feeds a write.
 */
export const DEFAULT_FIRST_SCENE_LABEL = "Default scene";

export function sceneDisplayLabel(label: string | undefined, index: number): string {
  const fallback = `Scene ${String.fromCharCode(65 + index)}`;
  const text = label?.trim() ? label : fallback;
  return index === 0 && text === fallback ? DEFAULT_FIRST_SCENE_LABEL : text;
}
