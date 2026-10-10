export interface QcLegalDocumentRecord {
  id: string;
  label: string;
  openLabel: string;
  bundledName: string;
  repositoryPath: string;
}
export declare const repositoryRoot: string;
export declare const legalDocumentsPath: string;
export declare function qcLegalDocuments(): QcLegalDocumentRecord[];
export declare function copyQcLegalDocuments(outputDirectory: string, root?: string): string[];
export declare function qcLegalAssets(options: { outputDirectory: string }): { name: string; closeBundle(): void };
