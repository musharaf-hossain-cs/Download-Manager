import {
  collectAllLinks,
  type AllLinksCollection,
  type AllLinksDocumentSource,
  type CollectAllLinksOptions,
  type LinkElementSource,
} from "./all-links.js";

export interface SelectedLinksContainerSource {
  querySelectorAll(
    selector: "a[href], area[href]",
  ): Iterable<LinkElementSource>;
}

export interface SelectedLinksRangeSource {
  cloneContents(): SelectedLinksContainerSource;
}

export interface SelectedLinksSelectionSource {
  readonly rangeCount: number;
  getRangeAt(index: number): SelectedLinksRangeSource;
}

export interface SelectedLinksDocumentSource {
  readonly baseURI: string;
  getSelection(): SelectedLinksSelectionSource | null;
}

/** Collects only links represented by the current user selection. */
export function collectSelectedLinks(
  document: SelectedLinksDocumentSource,
  options: CollectAllLinksOptions = {},
): Promise<AllLinksCollection> {
  const selection = document.getSelection();
  const selectedDocument: AllLinksDocumentSource = {
    baseURI: document.baseURI,
    querySelectorAll(selector) {
      if (selection === null) return [];
      return selectedElements(selection, selector);
    },
  };
  return collectAllLinks(selectedDocument, options);
}

function* selectedElements(
  selection: SelectedLinksSelectionSource,
  selector: "a[href], area[href]",
): IterableIterator<LinkElementSource> {
  for (let index = 0; index < selection.rangeCount; index += 1) {
    const fragment = selection.getRangeAt(index).cloneContents();
    yield* fragment.querySelectorAll(selector);
  }
}
