// Medium is the original 360×640 layout. Scale the whole scene once; the
// sprite keeps its medium CSS size so it is not scaled a second time.
export function sceneSize(scale, width, height) {
  const requested = Number.isFinite(scale) && scale > 0 ? Math.min(6, Math.max(1, scale)) / 2 : 1
  const factor = Math.min(requested, width / 360)
  return { factor, width: width / factor, height: height / factor }
}
