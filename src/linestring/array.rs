use crate::error::GeoArrowError;
use crate::slice::slice_validity_unchecked;
use crate::{GeometryArrayTrait, MultiPointArray};
use arrow2::array::{Array, ListArray, PrimitiveArray, StructArray};
use arrow2::bitmap::utils::{BitmapIter, ZipValidity};
use arrow2::bitmap::Bitmap;
use arrow2::buffer::Buffer;
use arrow2::datatypes::{DataType, Field};
use arrow2::offset::OffsetsBuffer;
use geozero::{GeomProcessor, GeozeroGeometry};
use rstar::RTree;

use super::MutableLineStringArray;

/// A [`GeometryArrayTrait`] semantically equivalent to `Vec<Option<LineString>>` using Arrow's
/// in-memory representation.
#[derive(Debug, Clone)]
pub struct LineStringArray {
    /// Buffer of x coordinates
    x: Buffer<f64>,

    /// Buffer of y coordinates
    y: Buffer<f64>,

    /// Offsets into the coordinate array where each geometry starts
    geom_offsets: OffsetsBuffer<i64>,

    /// Validity bitmap
    validity: Option<Bitmap>,
}

pub(super) fn check(
    x: &[f64],
    y: &[f64],
    validity_len: Option<usize>,
    geom_offsets: &OffsetsBuffer<i64>,
) -> Result<(), GeoArrowError> {
    // TODO: check geom offsets?
    if validity_len.map_or(false, |len| len != geom_offsets.len_proxy()) {
        return Err(GeoArrowError::General(
            "validity mask length must match the number of values".to_string(),
        ));
    }

    if x.len() != y.len() {
        return Err(GeoArrowError::General(
            "x and y arrays must have the same length".to_string(),
        ));
    }
    Ok(())
}

impl LineStringArray {
    /// Create a new LineStringArray from parts
    /// # Implementation
    /// This function is `O(1)`.
    pub fn new(
        x: Buffer<f64>,
        y: Buffer<f64>,
        geom_offsets: OffsetsBuffer<i64>,
        validity: Option<Bitmap>,
    ) -> Self {
        check(&x, &y, validity.as_ref().map(|v| v.len()), &geom_offsets).unwrap();
        Self {
            x,
            y,
            geom_offsets,
            validity,
        }
    }

    /// Create a new LineStringArray from parts
    /// # Implementation
    /// This function is `O(1)`.
    pub fn try_new(
        x: Buffer<f64>,
        y: Buffer<f64>,
        geom_offsets: OffsetsBuffer<i64>,
        validity: Option<Bitmap>,
    ) -> Result<Self, GeoArrowError> {
        check(&x, &y, validity.as_ref().map(|v| v.len()), &geom_offsets)?;
        Ok(Self {
            x,
            y,
            geom_offsets,
            validity,
        })
    }
}

impl<'a> GeometryArrayTrait<'a> for LineStringArray {
    type Scalar = crate::LineString<'a>;
    type ScalarGeo = geo::LineString;
    type ArrowArray = ListArray<i64>;

    /// Gets the value at slot `i`
    fn value(&'a self, i: usize) -> Self::Scalar {
        crate::LineString {
            x: &self.x,
            y: &self.y,
            geom_offsets: &self.geom_offsets,
            geom_index: i,
        }
    }

    fn into_arrow(self) -> ListArray<i64> {
        // Data type
        let coord_field_x = Field::new("x", DataType::Float64, false);
        let coord_field_y = Field::new("y", DataType::Float64, false);
        let struct_data_type = DataType::Struct(vec![coord_field_x, coord_field_y]);
        let list_data_type = DataType::LargeList(Box::new(Field::new(
            "vertices",
            struct_data_type.clone(),
            true,
        )));

        // Validity
        let validity: Option<Bitmap> = if let Some(validity) = self.validity {
            validity.into()
        } else {
            None
        };

        // Array data
        let array_x = PrimitiveArray::new(DataType::Float64, self.x, None).boxed();
        let array_y = PrimitiveArray::new(DataType::Float64, self.y, None).boxed();

        let coord_array = StructArray::new(struct_data_type, vec![array_x, array_y], None).boxed();

        ListArray::new(list_data_type, self.geom_offsets, coord_array, validity)
    }

    /// Build a spatial index containing this array's geometries
    fn rstar_tree(&'a self) -> RTree<Self::Scalar> {
        let mut tree = RTree::new();
        self.iter().flatten().for_each(|geom| tree.insert(geom));
        tree
    }

    /// Returns the number of geometries in this array
    #[inline]
    fn len(&self) -> usize {
        self.geom_offsets.len_proxy()
    }

    /// Returns the optional validity.
    #[inline]
    fn validity(&self) -> Option<&Bitmap> {
        self.validity.as_ref()
    }

    /// Slices this [`PrimitiveArray`] in place.
    /// # Implementation
    /// This operation is `O(1)`.
    /// # Examples
    /// ```
    /// use arrow2::array::PrimitiveArray;
    ///
    /// let array = PrimitiveArray::from_vec(vec![1, 2, 3]);
    /// assert_eq!(format!("{:?}", array), "Int32[1, 2, 3]");
    /// array.slice(1, 1);
    /// assert_eq!(format!("{:?}", array), "Int32[2]");
    /// ```
    /// # Panic
    /// This function panics iff `offset + length > self.len()`.
    #[inline]
    fn slice(&mut self, offset: usize, length: usize) {
        assert!(
            offset + length <= self.len(),
            "offset + length may not exceed length of array"
        );
        unsafe { self.slice_unchecked(offset, length) };
    }

    /// Slices this [`PrimitiveArray`] in place.
    /// # Implementation
    /// This operation is `O(1)`.
    /// # Safety
    /// The caller must ensure that `offset + length <= self.len()`.
    #[inline]
    unsafe fn slice_unchecked(&mut self, offset: usize, length: usize) {
        slice_validity_unchecked(&mut self.validity, offset, length);
        self.geom_offsets.slice_unchecked(offset, length + 1);
    }

    fn to_boxed(&self) -> Box<Self> {
        Box::new(self.clone())
    }
}

// Implement geometry accessors
impl LineStringArray {
    /// Iterator over geo Geometry objects, not looking at validity
    pub fn iter_geo_values(&self) -> impl Iterator<Item = geo::LineString> + '_ {
        (0..self.len()).map(|i| self.value_as_geo(i))
    }

    /// Iterator over geo Geometry objects, taking into account validity
    pub fn iter_geo(
        &self,
    ) -> ZipValidity<geo::LineString, impl Iterator<Item = geo::LineString> + '_, BitmapIter> {
        ZipValidity::new_with_validity(self.iter_geo_values(), self.validity())
    }

    /// Returns the value at slot `i` as a GEOS geometry.
    #[cfg(feature = "geos")]
    pub fn value_as_geos(&self, i: usize) -> geos::Geometry {
        (&self.value_as_geo(i)).try_into().unwrap()
    }

    /// Gets the value at slot `i` as a GEOS geometry, additionally checking the validity bitmap
    #[cfg(feature = "geos")]
    pub fn get_as_geos(&self, i: usize) -> Option<geos::Geometry> {
        if self.is_null(i) {
            return None;
        }

        self.get_as_geo(i).as_ref().map(|g| g.try_into().unwrap())
    }

    /// Iterator over GEOS geometry objects
    #[cfg(feature = "geos")]
    pub fn iter_geos_values(&self) -> impl Iterator<Item = geos::Geometry> + '_ {
        (0..self.len()).map(|i| self.value_as_geos(i))
    }

    /// Iterator over GEOS geometry objects, taking validity into account
    #[cfg(feature = "geos")]
    pub fn iter_geos(
        &self,
    ) -> ZipValidity<geos::Geometry, impl Iterator<Item = geos::Geometry> + '_, BitmapIter> {
        ZipValidity::new_with_validity(self.iter_geos_values(), self.validity())
    }
}

impl TryFrom<ListArray<i64>> for LineStringArray {
    type Error = GeoArrowError;

    fn try_from(value: ListArray<i64>) -> Result<Self, Self::Error> {
        let inner_dyn_array = value.values();
        let struct_array = inner_dyn_array
            .as_any()
            .downcast_ref::<StructArray>()
            .unwrap();
        let geom_offsets = value.offsets();
        let validity = value.validity();

        let x_array_values = struct_array.values()[0]
            .as_any()
            .downcast_ref::<PrimitiveArray<f64>>()
            .unwrap();
        let y_array_values = struct_array.values()[1]
            .as_any()
            .downcast_ref::<PrimitiveArray<f64>>()
            .unwrap();

        Ok(Self::new(
            x_array_values.values().clone(),
            y_array_values.values().clone(),
            geom_offsets.clone(),
            validity.cloned(),
        ))
    }
}

impl TryFrom<Box<dyn Array>> for LineStringArray {
    type Error = GeoArrowError;

    fn try_from(value: Box<dyn Array>) -> Result<Self, Self::Error> {
        let arr = value.as_any().downcast_ref::<ListArray<i64>>().unwrap();
        arr.clone().try_into()
    }
}

impl From<Vec<Option<geo::LineString>>> for LineStringArray {
    fn from(other: Vec<Option<geo::LineString>>) -> Self {
        let mut_arr: MutableLineStringArray = other.into();
        mut_arr.into()
    }
}

impl From<Vec<geo::LineString>> for LineStringArray {
    fn from(other: Vec<geo::LineString>) -> Self {
        let mut_arr: MutableLineStringArray = other.into();
        mut_arr.into()
    }
}

/// LineString and MultiPoint have the same layout, so enable conversions between the two to change
/// the semantic type
impl From<LineStringArray> for MultiPointArray {
    fn from(value: LineStringArray) -> Self {
        Self::new(value.x, value.y, value.geom_offsets, value.validity)
    }
}

impl GeozeroGeometry for LineStringArray {
    fn process_geom<P: GeomProcessor>(&self, processor: &mut P) -> geozero::error::Result<()>
    where
        Self: Sized,
    {
        let num_geometries = self.len();
        processor.geometrycollection_begin(num_geometries, 0)?;

        for geom_idx in 0..num_geometries {
            let (start_coord_idx, end_coord_idx) = self.geom_offsets.start_end(geom_idx);

            processor.linestring_begin(true, end_coord_idx - start_coord_idx, geom_idx)?;

            for coord_idx in start_coord_idx..end_coord_idx {
                processor.xy(
                    self.x[coord_idx],
                    self.y[coord_idx],
                    coord_idx - start_coord_idx,
                )?;
            }

            processor.linestring_end(true, geom_idx)?;
        }

        processor.geometrycollection_end(num_geometries - 1)?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use geo::{line_string, LineString};
    use geozero::ToWkt;
    use rstar::AABB;

    fn ls0() -> LineString {
        line_string![
            (x: 0., y: 1.),
            (x: 1., y: 2.)
        ]
    }

    fn ls1() -> LineString {
        line_string![
            (x: 3., y: 4.),
            (x: 5., y: 6.)
        ]
    }

    #[test]
    fn geo_roundtrip_accurate() {
        let arr: LineStringArray = vec![ls0(), ls1()].into();
        assert_eq!(arr.value_as_geo(0), ls0());
        assert_eq!(arr.value_as_geo(1), ls1());
    }

    #[test]
    fn geo_roundtrip_accurate_option_vec() {
        let arr: LineStringArray = vec![Some(ls0()), Some(ls1()), None].into();
        assert_eq!(arr.get_as_geo(0), Some(ls0()));
        assert_eq!(arr.get_as_geo(1), Some(ls1()));
        assert_eq!(arr.get_as_geo(2), None);
    }

    #[test]
    fn geozero_process_geom() -> geozero::error::Result<()> {
        let arr: LineStringArray = vec![ls0(), ls1()].into();
        let wkt = arr.to_wkt()?;
        let expected = "GEOMETRYCOLLECTION(LINESTRING(0 1,1 2),LINESTRING(3 4,5 6))";
        assert_eq!(wkt, expected);
        Ok(())
    }

    #[test]
    fn rstar_integration() {
        let arr: LineStringArray = vec![ls0(), ls1()].into();
        let tree = arr.rstar_tree();

        let search_box = AABB::from_corners([3.5, 5.5], [4.5, 6.5]);
        let results: Vec<&crate::LineString> =
            tree.locate_in_envelope_intersecting(&search_box).collect();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].geom_index, 1,
            "The second element in the LineStringArray should be found"
        );
    }

    #[test]
    fn slice() {
        let mut arr: LineStringArray = vec![ls0(), ls1()].into();
        arr.slice(1, 1);
        assert_eq!(arr.len(), 1);
        assert_eq!(arr.get_as_geo(0), Some(ls1()));
    }
}
