// Type declarations for lucide-react
declare module 'lucide-react' {
  import { FC, SVGProps } from 'react';
  
  interface IconProps extends SVGProps<SVGSVGElement> {
    size?: number | string;
    strokeWidth?: number | string;
    absoluteStrokeWidth?: boolean;
  }
  
  type Icon = FC<IconProps>;
  
  export const Folder: Icon;
  export const FolderOpen: Icon;
  export const FileText: Icon;
  export const FileCode: Icon;
  export const Settings: Icon;
  export const File: Icon;
  export const Image: Icon;
  export const Database: Icon;
  export const ChevronRight: Icon;
  export const ChevronDown: Icon;
  export const ArrowLeft: Icon;
  export const X: Icon;
  export const Loader2: Icon;
  export const AlertCircle: Icon;
  export const MessageSquare: Icon;
  export const Trash2: Icon;
  export const Send: Icon;
  export const ChevronUp: Icon;
}
