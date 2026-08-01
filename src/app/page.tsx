import { PerforatedRail } from "@/components/art/marks";
import { SplashArcs } from "@/components/art/PaintField";
import { Hero } from "@/components/sections/Hero";
import { Principles } from "@/components/sections/Principles";
import { Architecture } from "@/components/sections/Architecture";
import { BuildStatus } from "@/components/sections/BuildStatus";
import { Ownership } from "@/components/sections/Ownership";
import { Contribute } from "@/components/sections/Contribute";

/** A section joint: the machined perforated rail with color-splash arcs
 * bleeding across it, so the mural ignores the boundary between sections
 * while the rail keeps the grid legible. */
function MuralJoint({ id, railId }: { id: string; railId: string }) {
  return (
    <div className="relative overflow-x-clip">
      <SplashArcs
        id={id}
        className="paint-live pointer-events-none absolute -left-8 top-1/2 h-24 w-[120%] -translate-y-1/2 opacity-50"
      />
      <PerforatedRail id={railId} />
    </div>
  );
}

export default function HomePage() {
  return (
    <>
      <Hero />
      <MuralJoint id="joint-a" railId="rail-a" />
      <Principles />
      <Architecture />
      <MuralJoint id="joint-b" railId="rail-b" />
      <BuildStatus />
      <Ownership />
      <Contribute />
    </>
  );
}
