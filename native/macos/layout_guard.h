#pragma once
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

namespace linkdesk {
struct Rect {
    int64_t x, y, width, height;
    int64_t right() const { return x + width; }
    int64_t bottom() const { return y + height; }
    bool operator==(const Rect &r) const { return x==r.x && y==r.y && width==r.width && height==r.height; }
    bool valid() const {
        return width>0 && height>0 && width<=32768 && height<=32768 &&
            x>=INT32_MIN && y>=INT32_MIN && x<=INT32_MAX-width && y<=INT32_MAX-height;
    }
    bool intersects(const Rect &r) const {
        return x<r.right() && r.x<right() && y<r.bottom() && r.y<bottom();
    }
};
struct DisplayState {
    uint32_t id;
    Rect rect;
    uint64_t modeWidth, modeHeight, pixelWidth, pixelHeight;
    double refresh, rotation;
    uint32_t mirror;
    bool main, active;
    bool operator==(const DisplayState &d) const {
        return id==d.id && rect==d.rect && modeWidth==d.modeWidth && modeHeight==d.modeHeight &&
            pixelWidth==d.pixelWidth && pixelHeight==d.pixelHeight && refresh==d.refresh &&
            rotation==d.rotation && mirror==d.mirror && main==d.main && active==d.active;
    }
};

// This guard never returns a repair plan: only the owned virtual display may
// be configured or removed. A changed Sidecar/physical screen makes us abort.
inline bool existingUnchanged(const std::vector<DisplayState> &before,
                              const std::vector<DisplayState> &after, uint32_t owned) {
    if (before.empty() || after.size()!=before.size()+1 || !owned) return false;
    if (std::any_of(before.begin(),before.end(),[&](const auto &d){return d.id==owned;})) return false;
    if (std::count_if(after.begin(),after.end(),[&](const auto &d){return d.id==owned;})!=1) return false;
    for (const auto &d: before) {
        if (std::count_if(after.begin(),after.end(),[&](const auto &a){return a.id==d.id && a==d;})!=1) return false;
    }
    return true;
}

inline bool adjacent(const Rect &rect, const Rect &anchor, const std::string &side) {
    if (!rect.valid() || !anchor.valid()) return false;
    auto vertical=std::min(rect.bottom(),anchor.bottom())-std::max(rect.y,anchor.y);
    auto horizontal=std::min(rect.right(),anchor.right())-std::max(rect.x,anchor.x);
    return (side=="right" && rect.x==anchor.right() && vertical>=64) ||
        (side=="left" && rect.right()==anchor.x && vertical>=64) ||
        (side=="above" && rect.bottom()==anchor.y && horizontal>=64) ||
        (side=="below" && rect.y==anchor.bottom() && horizontal>=64);
}

inline bool placementValid(const std::vector<DisplayState> &existing, uint32_t anchor,
                           const std::string &side, const Rect &rect) {
    if (!rect.valid() || rect.width!=1920 || rect.height!=1080) return false;
    auto selected=std::find_if(existing.begin(),existing.end(),[&](const auto &d){return d.id==anchor && d.active && !d.mirror;});
    if (selected==existing.end() || !adjacent(rect,selected->rect,side)) return false;
    for (const auto &d:existing) if (d.active && !d.mirror && rect.intersects(d.rect)) return false;
    return true;
}
} // namespace linkdesk
