// Platform features the reader checks for; not all are in the DOM lib yet.
interface Document {
  prerendering?: boolean;
}

interface Navigator {
  connection?: { saveData?: boolean; effectiveType?: string };
  userAgentData?: { platform: string };
  /** iOS Safari's word for an installed web app. */
  standalone?: boolean;
}
