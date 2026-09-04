import { PerforatedRail } from "@/components/art/marks";
import { Masthead } from "@/components/sections/Masthead";
import { Shape } from "@/components/sections/Shape";
import { Path } from "@/components/sections/Path";
import { Measured } from "@/components/sections/Measured";
import { Open } from "@/components/sections/Open";

export default function HomePage() {
  return (
    <>
      <Masthead />
      <Shape />
      <PerforatedRail id="rail-a" />
      <Path />
      <Measured />
      <Open />
    </>
  );
}
