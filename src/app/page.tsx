import { PerforatedRail } from "@/components/art/marks";
import { Hero } from "@/components/sections/Hero";
import { Principles } from "@/components/sections/Principles";
import { Architecture } from "@/components/sections/Architecture";
import { BuildStatus } from "@/components/sections/BuildStatus";
import { Ownership } from "@/components/sections/Ownership";
import { Contribute } from "@/components/sections/Contribute";

export default function HomePage() {
  return (
    <>
      <Hero />
      <PerforatedRail id="rail-a" />
      <Principles />
      <Architecture />
      <PerforatedRail id="rail-b" />
      <BuildStatus />
      <Ownership />
      <Contribute />
    </>
  );
}
