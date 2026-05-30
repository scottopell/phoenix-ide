/**
 * Image body — renders an image inside the viewer shell. A first-class viewer
 * body, not a special case in the loader. Images are not annotatable, so this
 * carries no review-note wiring.
 */
export function ImageViewerBody({ fileName, url }: { fileName: string; url: string }) {
  return (
    <div className="image-preview">
      <img src={url} alt={fileName} className="image-preview-img" />
    </div>
  );
}
