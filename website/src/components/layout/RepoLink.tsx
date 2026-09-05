import { SITE_CONFIG } from "@/lib/site-config";

export function RepoLink({ className }: { className?: string }) {
  return (
    <a href={SITE_CONFIG.repoUrl} className={className}>
      GitHub (private) ↗
    </a>
  );
}
