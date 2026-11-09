// Exercises the same immutable-layout guard used by the native Mac helper.
// No display server, permissions, capture or virtual-display APIs are called.
#include "layout_guard.h"
#include <cstdlib>
#include <iostream>

static void check(bool result, const char *message) {
    if (!result) { std::cerr<<message<<'\n'; std::exit(1); }
}
int main() {
    using namespace linkdesk;
    DisplayState mac{1,{0,0,1728,1117},1728,1117,3456,2234,120,0,0,true,true};
    DisplayState sidecar{2,{300,-1024,1366,1024},1366,1024,2732,2048,60,0,0,false,true};
    DisplayState owned{3,{1666,-1080,1920,1080},1920,1080,1920,1080,60,0,0,false,true};
    std::vector<DisplayState> before{mac,sidecar}, after{mac,sidecar,owned};
    check(existingUnchanged(before,after,3),"unchanged screens must be accepted");
    check(placementValid(before,2,"right",owned.rect),"direct Sidecar edge must be accepted");
    check(!placementValid(before,2,"right",{1666,0,1920,1080}),"old main-aligned disconnected placement must fail");
    check(!placementValid(before,99,"right",owned.rect),"stale anchor must fail");
    for (int mutation=0;mutation<8;mutation++) {
        auto changed=after;
        switch (mutation) {
            case 0: changed[1].rect.x++; break;
            case 1: changed[1].pixelWidth++; break;
            case 2: changed[1].mirror=1; break;
            case 3: changed[1].rotation=90; break;
            case 4: changed[1].refresh=30; break;
            case 5: changed[1].modeWidth++; break;
            case 6: changed[1].active=false; break;
            case 7: changed[0].main=false; break;
        }
        check(!existingUnchanged(before,changed,3),"existing screen mutation must abort extension");
    }
    check(!existingUnchanged(before,{mac,owned},3),"disappearing Sidecar must abort extension");
    check(!existingUnchanged(before,{mac,sidecar,owned,owned},3),"unexpected display count must abort extension");
    check(!existingUnchanged(before,after,2),"an existing display cannot be owned");
    auto mirrored=before; mirrored[1].mirror=1;
    check(!placementValid(mirrored,2,"right",owned.rect),"mirrored Sidecar cannot anchor an independent passage");
    auto occupied=before; occupied[1].rect={1728,0,1366,1024};
    check(!placementValid(occupied,1,"right",{1728,0,1920,1080}),"overlap with Sidecar must fail");
    std::cout<<"PASS: native existing-display protection and adjacency guards\n";
}
