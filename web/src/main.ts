import './reader';

if (import.meta.hot) {
  // Reader services own document-level listeners; reload when their module changes.
  import.meta.hot.accept('./reader', () => location.reload());
}
